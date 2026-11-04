//! 后台导出任务：启动 ffmpeg、读进度、可取消。
//!
//! 进度与错误日志共用一个管道（`-progress pipe:2`）：`-loglevel error` 之后
//! stderr 上只剩错误行与进度行，一个读线程就能同时喂进度条和失败原因。

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use futures::channel::mpsc::UnboundedSender;

use crate::platform::ffmpeg;
use crate::timeline::{ExportPlan, Tick};

/// 失败时回传的日志行数：够看清原因，又不至于把界面撑爆。
const TAIL_LINES: usize = 12;

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
        plan: ExportPlan,
        output: PathBuf,
        total: Tick,
        sender: UnboundedSender<ExportEvent>,
    ) -> anyhow::Result<Self> {
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
            .args(plan.args())
            .arg(&output)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        let mut child = command.spawn()?;
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
}
