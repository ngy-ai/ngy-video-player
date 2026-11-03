//! 镜头分割：扫一遍整片，找出画面突然切换的时间点。
//!
//! 检测交给 ffmpeg 的 `select=gt(scene,阈值)` + `showinfo`：匹配到的每一帧都会在
//! stderr 里打一行 `pts_time`。这里负责拼命令、解析时间点与进度，以及取消。
//!
//! 检测要解码整片，长片会跑上一阵，所以进度与取消必须一起给。切点按发现顺序
//! （也就是时间先后）逐个回报，界面可以边扫边列，不用等到最后。

use anyhow::{Context as _, Result};
use futures::channel::mpsc::UnboundedSender;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 默认阈值。太低会把渐变转场当成切换，太高会漏掉快切。
pub const DEFAULT_THRESHOLD: f32 = 0.35;

pub struct SceneRequest {
    pub input: PathBuf,
    /// 原片时长，用来折算进度。0 表示还没读出来，进度条停在 0。
    pub duration: f64,
    /// `scene` 分数阈值，0.0–1.0。
    pub threshold: f32,
}

/// 后台任务回报给界面的事件。
#[derive(Clone, Debug)]
pub enum SceneEvent {
    /// 0.0–1.0。
    Progress(f32),
    /// 发现一个切点（秒）。可能是负数（B 帧重排），界面自己夹到 0。
    Found(f64),
    Done(SceneOutcome),
}

#[derive(Clone, Debug)]
pub enum SceneOutcome {
    Finished,
    Cancelled,
    Failed(String),
}

/// 正在跑的 ffmpeg。界面只持有句柄：进度与切点走事件通道，取消直接杀进程。
pub struct SceneJob {
    child: Arc<Mutex<Option<Child>>>,
    cancelled: Arc<AtomicBool>,
}

impl SceneJob {
    /// 启动后台检测。进程启动失败会立即返回错误。
    pub fn start(request: SceneRequest, tx: UnboundedSender<SceneEvent>) -> Result<Self> {
        let executable = crate::platform::ffmpeg::executable()?;
        let mut child = build_command(&executable, &request)
            .spawn()
            .with_context(|| format!("启动 ffmpeg 失败：{}", executable.display()))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let child = Arc::new(Mutex::new(Some(child)));
        let cancelled = Arc::new(AtomicBool::new(false));

        // showinfo 把切点打在 stderr 上，这条管道不能像别处那样只当错误日志：必须
        // 一直读走，否则管道塞满会让 ffmpeg 卡住。顺手留最后几行当失败原因。
        let tail = Arc::new(Mutex::new(Vec::<String>::new()));
        if let Some(stderr) = stderr {
            let tail = tail.clone();
            let tx = tx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(|line| line.ok()) {
                    if let Some(time) = parse_pts_time(&line) {
                        let _ = tx.unbounded_send(SceneEvent::Found(time));
                        continue;
                    }
                    let mut tail = tail.lock().unwrap();
                    if tail.len() >= 8 {
                        tail.remove(0);
                    }
                    tail.push(line);
                }
            });
        }

        let task_child = child.clone();
        let task_cancelled = cancelled.clone();
        let total = request.duration;
        std::thread::spawn(move || {
            if let Some(stdout) = stdout {
                read_progress(stdout, total, &tx);
            }
            let status = crate::platform::ffmpeg::wait_polling(&task_child);
            let outcome = if task_cancelled.load(Ordering::SeqCst) {
                SceneOutcome::Cancelled
            } else {
                match status {
                    Some(Ok(status)) if status.success() => SceneOutcome::Finished,
                    Some(Ok(status)) => {
                        let detail = tail.lock().unwrap().join("\n");
                        SceneOutcome::Failed(if detail.is_empty() {
                            format!("ffmpeg 异常退出（{:?}）", status.code())
                        } else {
                            detail
                        })
                    }
                    Some(Err(error)) => {
                        SceneOutcome::Failed(format!("等待 ffmpeg 结束失败：{error}"))
                    }
                    None => SceneOutcome::Failed("ffmpeg 进程已经结束".to_string()),
                }
            };
            let _ = tx.unbounded_send(SceneEvent::Done(outcome));
        });

        Ok(Self { child, cancelled })
    }

    /// 取消检测。子进程会被杀掉，任务线程随后回报 `Cancelled`。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
    }
}

impl Drop for SceneJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// 拼 ffmpeg 参数。
fn build_command(executable: &Path, request: &SceneRequest) -> Command {
    let mut command = Command::new(executable);
    command
        .arg("-hide_banner")
        .arg("-nostdin")
        .arg("-nostats")
        // showinfo 打的是 info 级别日志，把 loglevel 压成 error 就一条切点都收不到。
        .arg("-loglevel")
        .arg("info")
        .arg("-progress")
        .arg("pipe:1")
        .arg("-i")
        .arg(&request.input)
        .arg("-filter:v")
        .arg(format!(
            "select='gt(scene,{:.3})',showinfo",
            request.threshold.clamp(0.0, 1.0)
        ))
        // 只关心画面，音频与字幕解码纯属浪费。
        .arg("-an")
        .arg("-sn")
        .arg("-f")
        .arg("null")
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// 从 showinfo 的一行里取出 `pts_time`。
fn parse_pts_time(line: &str) -> Option<f64> {
    let marker = "pts_time:";
    let start = line.find(marker)? + marker.len();
    let text: String = line[start..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || matches!(ch, '.' | '-' | '+'))
        .collect();
    if text.is_empty() || !text.chars().any(|ch| ch.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// 从 `-progress pipe:1` 的输出里读进度。
fn read_progress(stdout: impl Read, total: f64, tx: &UnboundedSender<SceneEvent>) {
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
        let _ = tx.unbounded_send(SceneEvent::Progress(progress));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 从showinfo行里取时间点() {
        let line = "[Parsed_showinfo_1 @ 00000255f0e23300] n:   0 pts:  25600 pts_time:2       \
                    duration:    512 duration_time:0.04";
        assert_eq!(parse_pts_time(line), Some(2.0));
        assert_eq!(parse_pts_time("pts_time:12.345 dur"), Some(12.345));
        // B 帧重排时可能出现负时间点，解析不能把它丢掉。
        assert_eq!(parse_pts_time("pts_time:-0.04"), Some(-0.04));
        assert_eq!(parse_pts_time("[out @ 0x0] frame=  100"), None);
        assert_eq!(parse_pts_time("pts_time:"), None);
    }

    #[test]
    fn 命令带阈值且不压日志级别() {
        let request = SceneRequest {
            input: PathBuf::from("movie.mp4"),
            duration: 600.0,
            threshold: DEFAULT_THRESHOLD,
        };
        let args: Vec<String> = build_command(Path::new("ffmpeg"), &request)
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(args.iter().any(|arg| arg == "-loglevel"));
        // 阈值写在滤镜里，0.35 是默认档。
        assert!(args.iter().any(|arg| arg.contains("gt(scene,0.350)")));
        assert!(args.iter().any(|arg| arg.contains("showinfo")));
        assert!(args.windows(2).any(|pair| pair == ["-f", "null"]));
    }
}
