//! 后台导出任务：启动 ffmpeg、读进度、可取消。
//!
//! 进度与错误日志共用一个管道（`-progress pipe:2`）：`-loglevel error` 之后
//! stderr 上只剩错误行与进度行，一个读线程就能同时喂进度条和失败原因。

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures::channel::mpsc::UnboundedSender;

use crate::platform::ffmpeg;
use crate::timeline::{ExportPlan, SubtitleMode, Tick};

/// 失败时回传的日志行数：够看清原因，又不至于把界面撑爆。
const TAIL_LINES: usize = 12;

/// 同一进程里排着几条导出时，临时 ASS 文件名也要各不相同。
static SUBTITLE_SEQ: AtomicU64 = AtomicU64::new(0);

/// 遮罩 alpha 图（PGM）的临时文件序号，理由同 `SUBTITLE_SEQ`。
static MASK_SEQ: AtomicU64 = AtomicU64::new(0);

/// 导出任务回报。
#[derive(Clone, Debug, PartialEq)]
pub enum ExportEvent {
    /// 0..=1。
    Progress(f32),
    Finished(PathBuf),
    Failed(String),
    Cancelled,
}

/// 一次导出任务。丢掉句柄不会杀掉进程，要停就显式调 [`ExportTask::cancel`]。
pub struct ExportTask {
    child: Arc<Mutex<Option<Child>>>,
    cancelled: Arc<AtomicBool>,
}

impl ExportTask {
    /// 启动 ffmpeg。`total` 是序列时长，用来把 `out_time_us` 换算成比例。
    pub fn start(
        mut plan: ExportPlan,
        output: PathBuf,
        total: Tick,
        sender: UnboundedSender<ExportEvent>,
    ) -> anyhow::Result<Self> {
        // 遮罩图与字幕文本同理：字节在计划里，文件到任务启动时才写。
        // 写完的路径替换掉计划里预留的那些占位输入，滤镜图里的 `[N:v]` 才对得上。
        let mask_paths = write_masks(&plan)?;
        plan.resolve_masks(&mask_paths);
        // 字幕文本只在计划里，真正落盘要等任务启动：队列里排着的几条导出不能
        // 抢同一个临时文件。写完的路径同时决定 `-i` 与 `subtitles=` 滤镜参数。
        let subtitle = match plan.subtitle.as_ref() {
            Some(attachment) => Some((
                write_subtitle(&attachment.ass)?,
                attachment.mode,
                subtitle_codec(&output),
            )),
            None => None,
        };
        let args = subtitle_args(&plan, &subtitle);
        let subtitle_path = subtitle.as_ref().map(|(path, _, _)| path.clone());
        let cleanup: Vec<PathBuf> = mask_paths
            .iter()
            .cloned()
            .chain(subtitle_path.clone())
            .collect();

        let executable = ffmpeg::executable()?;
        let mut command = Command::new(executable);
        command
            .arg("-hide_banner")
            .arg("-nostdin")
            .arg("-y")
            .arg("-nostats")
            .arg("-loglevel")
            .arg("error")
            .arg("-progress")
            .arg("pipe:2")
            .args(args)
            .arg(&output)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                for path in &cleanup {
                    let _ = std::fs::remove_file(path);
                }
                return Err(error.into());
            }
        };
        let stderr = child.stderr.take();
        let slot = Arc::new(Mutex::new(Some(child)));
        let cancelled = Arc::new(AtomicBool::new(false));

        let task_slot = slot.clone();
        let task_cancelled = cancelled.clone();
        let total_seconds = total.secs();
        std::thread::spawn(move || {
            let mut tail = VecDeque::new();
            if let Some(stderr) = stderr {
                drain(stderr, total_seconds, &sender, &mut tail);
            }
            let status = ffmpeg::wait_polling(&task_slot);

            let outcome = if task_cancelled.load(Ordering::SeqCst) {
                // 半成品留着只会让用户以为导出成功了。
                let _ = std::fs::remove_file(&output);
                ExportEvent::Cancelled
            } else {
                match status {
                    Some(Ok(status)) if status.success() => ExportEvent::Finished(output.clone()),
                    Some(Ok(status)) => ExportEvent::Failed(describe(status.code(), &tail)),
                    Some(Err(error)) => {
                        ExportEvent::Failed(format!("等待 ffmpeg 结束失败：{error}"))
                    }
                    None => ExportEvent::Failed("ffmpeg 进程已经结束".to_string()),
                }
            };
            // 成败都把临时 ASS 与遮罩图收掉：它们只对这一次 ffmpeg 进程有意义。
            for path in &cleanup {
                let _ = std::fs::remove_file(path);
            }
            let _ = sender.unbounded_send(outcome);
        });

        Ok(Self {
            child: slot,
            cancelled,
        })
    }

    /// 取消导出。子进程会在几百毫秒内被回收，任务线程负责删掉半成品。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn drain(
    stderr: impl Read,
    total_seconds: f64,
    sender: &UnboundedSender<ExportEvent>,
    tail: &mut VecDeque<String>,
) {
    for line in BufReader::new(stderr).lines().map_while(|line| line.ok()) {
        if let Some(fraction) = progress_of(&line, total_seconds) {
            let _ = sender.unbounded_send(ExportEvent::Progress(fraction));
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        if tail.len() >= TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(line);
    }
}

/// 解析一行进度。`out_time_us` 是微秒，`-1` 表示还没开始。
fn progress_of(line: &str, total_seconds: f64) -> Option<f32> {
    if total_seconds <= 0.0 {
        return None;
    }
    let value = line.strip_prefix("out_time_us=")?;
    let micros: f64 = value.trim().parse().ok()?;
    if micros < 0.0 {
        return None;
    }
    Some((micros / 1_000_000.0 / total_seconds).clamp(0.0, 1.0) as f32)
}

fn describe(code: Option<i32>, tail: &VecDeque<String>) -> String {
    let detail = tail.iter().cloned().collect::<Vec<_>>().join("\n");
    if detail.is_empty() {
        format!("ffmpeg 异常退出（{code:?}）")
    } else {
        format!("ffmpeg 异常退出（{code:?}）\n{detail}")
    }
}

/// 计划里带字幕时补上临时 ASS 文件对应的参数；没字幕就还是原来那套。
fn subtitle_args(
    plan: &ExportPlan,
    subtitle: &Option<(PathBuf, SubtitleMode, &'static str)>,
) -> Vec<String> {
    match subtitle {
        Some((path, mode, codec)) => plan.args_with_subtitle(path, *mode, codec),
        None => plan.args(),
    }
}

/// 把计划里的遮罩 alpha 图写成 PGM 临时文件。
///
/// 与空输入位一一对应：调用方随后用 [`ExportPlan::resolve_masks`] 把它们塞回
/// `inputs` 的占位位置。没有遮罩时返回空表，命令里也不会多出 `-i`。
fn write_masks(plan: &ExportPlan) -> std::io::Result<Vec<PathBuf>> {
    let mut paths = Vec::with_capacity(plan.masks.len());
    for attachment in &plan.masks {
        let seq = MASK_SEQ.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("ngy-mask-{}-{seq}.pgm", std::process::id()));
        std::fs::write(&path, &attachment.pgm)?;
        paths.push(path);
    }
    Ok(paths)
}

/// ASS 文本落到临时文件。`SUBTITLE_SEQ` 保证同进程并发导出也不会互相覆盖。
fn write_subtitle(ass: &str) -> std::io::Result<PathBuf> {
    let seq = SUBTITLE_SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("ngy-sub-{}-{seq}.ass", std::process::id()));
    std::fs::write(&path, ass.as_bytes())?;
    Ok(path)
}

/// 软字幕的编码器取决于容器：mkv 能原样装 ASS，mp4 / mov 只认 mov_text
/// （样式会丢一部分，见 README「已知限制」）。
fn subtitle_codec(output: &Path) -> &'static str {
    match output.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("mkv") => "ass",
        _ => "mov_text",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 进度行换算成比例并夹住() {
        assert_eq!(progress_of("out_time_us=5000000", 10.0), Some(0.5));
        assert_eq!(progress_of("out_time_us=0", 10.0), Some(0.0));
        assert_eq!(
            progress_of("out_time_us=20000000", 10.0),
            Some(1.0),
            "超出总长也只到 100%"
        );
        assert_eq!(progress_of("out_time_us=-1", 10.0), None, "-1 表示还没开始");
        assert_eq!(progress_of("out_time_us=5000000", 0.0), None, "总时长未知");
        assert_eq!(progress_of("frame=12", 10.0), None, "别的行不认");
    }

    #[test]
    fn 失败信息带上日志尾巴() {
        let mut tail = VecDeque::new();
        tail.push_back("Error opening input".to_string());
        let message = describe(Some(1), &tail);
        assert!(message.contains("Error opening input"), "{message}");
        assert!(message.contains("Some(1)"), "{message}");

        assert_eq!(describe(None, &VecDeque::new()), "ffmpeg 异常退出（None）");
    }

    #[test]
    fn 软字幕编码器按容器挑() {
        assert_eq!(subtitle_codec(Path::new("成片.mkv")), "ass");
        assert_eq!(subtitle_codec(Path::new("成片.MKV")), "ass");
        assert_eq!(subtitle_codec(Path::new("成片.mp4")), "mov_text");
        assert_eq!(subtitle_codec(Path::new("成片")), "mov_text");
    }

    #[test]
    fn 临时字幕文件每次都独立且内容正确() {
        let first = write_subtitle("第一条").expect("写临时字幕");
        let second = write_subtitle("第二条").expect("写临时字幕");
        assert_ne!(first, second);
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "第一条");
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "第二条");
        let _ = std::fs::remove_file(&first);
        let _ = std::fs::remove_file(&second);
    }
}
