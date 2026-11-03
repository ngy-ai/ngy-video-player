//! 拼接：把多个文件按顺序接成一个。
//!
//! 走 concat 分离器 + 流复制：不重新编码，几十个文件也是几秒钟。代价是要求每个
//! 文件的编码参数（编码器、分辨率、帧率、声道数）一致——不然接缝处会花屏，或者
//! ffmpeg 直接拒绝。参数不一致的文件先拿「格式转换」统一一遍再来拼。

use super::cutout::{drain_stderr, failure_message};
use super::*;
use std::fmt::Write as _;

/// 一次拼接请求。
#[derive(Clone, Debug)]
pub struct ConcatRequest {
    pub inputs: Vec<PathBuf>,
    pub output: PathBuf,
}

#[derive(Clone, Debug)]
pub enum ConcatEvent {
    Progress(f32),
    Done(ConcatOutcome),
}

#[derive(Clone, Debug)]
pub enum ConcatOutcome {
    Finished(PathBuf),
    Cancelled,
    Failed(String),
}

/// 正在跑的拼接任务。取消就是杀掉 ffmpeg 并删掉半成品。
pub struct ConcatJob {
    child: Arc<Mutex<Option<Child>>>,
    cancelled: Arc<AtomicBool>,
}

impl ConcatJob {
    pub fn start(request: ConcatRequest, tx: UnboundedSender<ConcatEvent>) -> Result<Self> {
        if request.inputs.len() < 2 {
            anyhow::bail!("至少要两个文件才能拼接");
        }
        let executable = crate::platform::ffmpeg::executable()?;

        // 时长只用来算进度；探不出来也不影响拼接本身，退化成「不确定进度」即可。
        let total: f64 = request
            .inputs
            .iter()
            .map(|input| probe_duration(&executable, input).unwrap_or(0.0))
            .sum();

        let list = std::env::temp_dir().join(format!("ngy-join-{}.txt", std::process::id()));
        let mut text = String::new();
        for input in &request.inputs {
            // concat 分离器按行解析：反斜杠换成正斜杠，单引号按它的规则转义。
            let path = input
                .to_string_lossy()
                .replace('\\', "/")
                .replace('\'', "'\\''");
            let _ = writeln!(text, "file '{path}'");
        }
        std::fs::write(&list, text)
            .with_context(|| format!("写临时清单失败：{}", list.display()))?;

        let mut command = Command::new(&executable);
        command
            .arg("-hide_banner")
            .arg("-nostdin")
            .arg("-y")
            .arg("-loglevel")
            .arg("error")
            .arg("-progress")
            .arg("pipe:1")
            .arg("-f")
            .arg("concat")
            .arg("-safe")
            .arg("0")
            .arg("-i")
            .arg(&list)
            .arg("-c")
            .arg("copy")
            .arg("-avoid_negative_ts")
            .arg("make_zero")
            .arg(&request.output)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command
            .spawn()
            .with_context(|| format!("启动 ffmpeg 失败：{}", executable.display()))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let slot = Arc::new(Mutex::new(Some(child)));
        let cancelled = Arc::new(AtomicBool::new(false));

        let task_slot = slot.clone();
        let task_cancelled = cancelled.clone();
        let output = request.output.clone();
        std::thread::spawn(move || {
            let tail = drain_stderr(stderr);
            if let Some(stdout) = stdout {
                read_progress(stdout, total, &tx);
            }
            let status = crate::platform::ffmpeg::wait_polling(&task_slot);
            // 清单是这次任务的中间产物，无论成败都删掉。
            let _ = std::fs::remove_file(&list);

            let outcome = if task_cancelled.load(Ordering::SeqCst) {
                remove_output(&output);
                ConcatOutcome::Cancelled
            } else {
                match status {
                    Some(Ok(status)) if status.success() => ConcatOutcome::Finished(output),
                    other => {
                        remove_output(&output);
                        ConcatOutcome::Failed(failure_message(other, &tail))
                    }
                }
            };
            let _ = tx.unbounded_send(ConcatEvent::Done(outcome));
        });

        Ok(Self {
            child: slot,
            cancelled,
        })
    }

    /// 取消。正在跑的 ffmpeg 会被杀掉，任务线程负责删掉半成品并回报结果。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// 从 `-progress pipe:1` 的输出里读进度。
fn read_progress(stdout: impl Read, total: f64, tx: &UnboundedSender<ConcatEvent>) {
    let total_us = total * 1_000_000.0;
    if total_us <= 0.0 {
        return;
    }
    for line in BufReader::new(stdout).lines().map_while(|line| line.ok()) {
        let Some(value) = line.strip_prefix("out_time_us=") else {
            continue;
        };
        let Ok(micros) = value.trim().parse::<f64>() else {
            continue;
        };
        let progress = (micros / total_us).clamp(0.0, 1.0) as f32;
        let _ = tx.unbounded_send(ConcatEvent::Progress(progress));
    }
}

/// 探测一个文件的时长（秒）。读 `Duration: HH:MM:SS.ss` 那一行。
fn probe_duration(executable: &Path, input: &Path) -> Option<f64> {
    let output = Command::new(executable)
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(input)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stderr).into_owned();
    parse_duration(&text)
}

/// 从 ffmpeg 的探测输出里读出时长。没有时长（比如损坏的文件）时返回 `None`。
fn parse_duration(text: &str) -> Option<f64> {
    let start = text.find("Duration: ")? + "Duration: ".len();
    // 形如 `00:01:02.34`，固定 11 个字符；不足说明格式不对，直接放弃。
    let stamp = text.get(start..start + 11)?;
    let mut parts = stamp.split(':');
    let hours: f64 = parts.next()?.parse().ok()?;
    let minutes: f64 = parts.next()?.parse().ok()?;
    let seconds: f64 = parts.next()?.parse().ok()?;
    Some(hours * 3600.0 + minutes * 60.0 + seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 从探测输出里读时长() {
        let text = "  Duration: 00:01:02.34, start: 0.000000, bitrate: 1234 kb/s";
        assert_eq!(parse_duration(text), Some(62.34));
    }

    #[test]
    fn 没有时长时不猜() {
        assert_eq!(parse_duration("Input #0, matroska"), None);
        assert_eq!(parse_duration("Duration: N/A"), None);
    }
}
