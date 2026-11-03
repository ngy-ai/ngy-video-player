//! 多段任务：「删除中间段」与「分割」。
//!
//! 前者去掉选定区间、把前后两段拼成一个文件；后者在切点处把片子分成两个文件。
//! 两者都是「先逐段编码，再决定怎么收尾」：删除中间段写临时文件再 concat，分割
//! 直接写到各自的落点。
//!
//! 这两条路一定重编码：被切出来的段大多不从关键帧起头，流复制接不上，接缝会花屏。

use super::export;
use super::job::{read_progress_mapped, remove_output};
use super::{
    ClipEvent, ClipJob, ClipOutcome, ClipRange, ClipRequest, CutKind, MIN_DURATION,
    split_output_paths,
};
use anyhow::{Context as _, Result};
use futures::channel::mpsc::UnboundedSender;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 去掉 `range` 之后还剩下的段落。区间顶到两端时只剩一段。
pub(super) fn segments(range: ClipRange, duration: f64) -> Vec<ClipRange> {
    let mut parts = Vec::new();
    // 半帧以内的残留不算「还有一段」：切出来的空片段会让 ffmpeg 直接报错。
    if range.start > MIN_DURATION {
        parts.push(ClipRange {
            start: 0.0,
            end: range.start,
        });
    }
    if range.end < duration - MIN_DURATION {
        parts.push(ClipRange {
            start: range.end,
            end: duration,
        });
    }
    parts
}

/// 在切点处把片子分成前后两段。切点贴着两端时不产生空片段。
pub(super) fn split_segments(range: ClipRange, duration: f64) -> Vec<ClipRange> {
    if range.start <= MIN_DURATION || range.start >= duration - MIN_DURATION {
        return Vec::new();
    }
    vec![
        ClipRange {
            start: 0.0,
            end: range.start,
        },
        ClipRange {
            start: range.start,
            end: duration,
        },
    ]
}

/// 启动多段任务。`burn` 是已经抽好的字幕文件，各段共用同一份。
pub(super) fn start(
    executable: &Path,
    request: ClipRequest,
    encoder: &'static str,
    burn: Option<PathBuf>,
    tx: UnboundedSender<ClipEvent>,
) -> Result<ClipJob> {
    let (parts, targets, concat_to) = if request.kind == CutKind::Split {
        let parts = split_segments(request.range, request.duration);
        if parts.is_empty() {
            anyhow::bail!("切点要落在片子的中间，不能贴着头尾");
        }
        let (first, second) = split_output_paths(&request.input);
        (parts, vec![first, second], None)
    } else {
        let parts = segments(request.range, request.duration);
        if parts.is_empty() {
            anyhow::bail!("这个区间就是整条视频，删掉之后什么都不剩");
        }
        // 删除中间段要先写临时文件：两段拼完才能得到最终产物。
        let temps: Vec<PathBuf> = (0..parts.len())
            .map(|index| {
                std::env::temp_dir().join(format!("ngy-cut-{}-{index}.mkv", std::process::id()))
            })
            .collect();
        (parts, temps, Some(request.output.clone()))
    };

    let child = Arc::new(Mutex::new(None));
    let cancelled = Arc::new(AtomicBool::new(false));
    let job_child = child.clone();
    let job_cancelled = cancelled.clone();
    let executable = executable.to_path_buf();

    std::thread::spawn(move || {
        let task = Task {
            executable: &executable,
            encoder,
            burn: burn.as_deref(),
            child: &job_child,
            cancelled: &job_cancelled,
            tx: &tx,
        };
        let outcome = match run(&request, &parts, &targets, concat_to.as_deref(), &task) {
            Ok(paths) => ClipOutcome::Finished(paths),
            Err(error) => {
                remove_output(&request.output);
                if job_cancelled.load(Ordering::SeqCst) {
                    ClipOutcome::Cancelled
                } else {
                    ClipOutcome::Failed(format!("{error:#}"))
                }
            }
        };
        // 临时字幕是这次任务的中间产物，收尾时清掉。
        if let Some(path) = &burn {
            let _ = fs::remove_file(path);
        }
        let _ = tx.unbounded_send(ClipEvent::Done(outcome));
    });

    Ok(ClipJob::new(child, cancelled, None))
}

/// 多段任务运行时共用的东西。凑成一个结构，免得每处都要传七八个参数。
struct Task<'a> {
    executable: &'a Path,
    encoder: &'a str,
    burn: Option<&'a Path>,
    child: &'a Arc<Mutex<Option<Child>>>,
    cancelled: &'a AtomicBool,
    tx: &'a UnboundedSender<ClipEvent>,
}

/// 依次编码每一段，最后按需要拼接。进度按各段的时长折算到整单上。
fn run(
    request: &ClipRequest,
    parts: &[ClipRange],
    targets: &[PathBuf],
    concat_to: Option<&Path>,
    task: &Task<'_>,
) -> Result<Vec<PathBuf>> {
    let executable = task.executable;
    let encoder = task.encoder;
    let burn = task.burn;
    let child = task.child;
    let cancelled = task.cancelled;
    let tx = task.tx;
    let total: f64 = parts.iter().map(|part| part.duration()).sum();
    let mut written: Vec<PathBuf> = Vec::new();
    let mut elapsed = 0.0;

    for (index, part) in parts.iter().enumerate() {
        if cancelled.load(Ordering::SeqCst) {
            discard(&written);
            anyhow::bail!("已取消");
        }
        let target = &targets[index];

        let mut segment = request.clone();
        segment.range = *part;
        segment.output = target.clone();

        let mut spawned = export::build_command(executable, &segment, encoder, burn)
            .spawn()
            .with_context(|| format!("启动 ffmpeg 失败：{}", executable.display()))?;
        let stdout = spawned.stdout.take();
        let tail = drain_stderr(spawned.stderr.take());
        *child.lock().unwrap() = Some(spawned);

        let base = (elapsed / total) as f32;
        let weight = (part.duration() / total) as f32;
        if let Some(stdout) = stdout {
            read_progress_mapped(
                stdout,
                part.duration(),
                |fraction| base + fraction * weight,
                tx,
            );
        }

        let status = crate::platform::ffmpeg::wait_polling(child);
        elapsed += part.duration();
        match status {
            Some(Ok(status)) if status.success() => {}
            other => {
                // 失败时把已经写出来的产物也删掉，别留半套让人以为导出了一半。
                discard(&written);
                let _ = fs::remove_file(target);
                anyhow::bail!(failure_message(other, &tail));
            }
        }
        written.push(target.clone());

        if cancelled.load(Ordering::SeqCst) {
            discard(&written);
            anyhow::bail!("已取消");
        }
    }

    let _ = tx.unbounded_send(ClipEvent::Progress(1.0));
    match concat_to {
        Some(output) => {
            concat(executable, &written, output)?;
            // 临时分段只有在拼完之后才能删。
            discard(&written);
            Ok(vec![output.to_path_buf()])
        }
        None => Ok(written),
    }
}

/// 拼接：两段编码参数完全一致，直接复制流。
fn concat(executable: &Path, parts: &[PathBuf], output: &Path) -> Result<()> {
    let list = std::env::temp_dir().join(format!("ngy-cut-{}-list.txt", std::process::id()));
    let mut text = String::new();
    for part in parts {
        // concat 分离器按行解析：反斜杠换成正斜杠，单引号按它的规则转义。
        let path = part
            .to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "'\\''");
        let _ = writeln!(text, "file '{path}'");
    }
    fs::write(&list, text).with_context(|| format!("写临时清单失败：{}", list.display()))?;

    let result = Command::new(executable)
        .arg("-hide_banner")
        .arg("-nostdin")
        .arg("-y")
        .arg("-loglevel")
        .arg("error")
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
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("启动 ffmpeg 拼接失败：{}", executable.display()))?;
    let _ = fs::remove_file(&list);

    if !result.status.success() {
        let detail = String::from_utf8_lossy(&result.stderr);
        anyhow::bail!("拼接失败：{}", detail.trim());
    }
    Ok(())
}

/// 收集 ffmpeg 的 stderr 末尾若干行，失败时给用户看。
pub(super) fn drain_stderr(
    stderr: Option<std::process::ChildStderr>,
) -> Arc<Mutex<VecDeque<String>>> {
    let tail = Arc::new(Mutex::new(VecDeque::new()));
    let Some(stderr) = stderr else {
        return tail;
    };
    let sink = tail.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(|line| line.ok()) {
            let mut tail = sink.lock().unwrap();
            // 只留最后十几行：ffmpeg 的报错总在收尾处，前面的都是进度噪音。
            if tail.len() >= 12 {
                tail.pop_front();
            }
            tail.push_back(line);
        }
    });
    tail
}

pub(super) fn failure_message(
    status: Option<std::io::Result<ExitStatus>>,
    tail: &Mutex<VecDeque<String>>,
) -> String {
    let detail = tail
        .lock()
        .unwrap()
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    let head = match status {
        Some(Ok(status)) => match status.code() {
            Some(code) => format!("ffmpeg 异常退出（{code}）"),
            None => "ffmpeg 被信号终止".to_string(),
        },
        Some(Err(error)) => return format!("等待 ffmpeg 结束失败：{error}"),
        None => "ffmpeg 进程已经结束".to_string(),
    };
    if detail.is_empty() {
        head
    } else {
        format!("{head}\n{detail}")
    }
}

/// 删掉已经写出的中间产物 / 半成品。
fn discard(parts: &[PathBuf]) {
    for part in parts {
        let _ = fs::remove_file(part);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: f64, end: f64) -> ClipRange {
        ClipRange { start, end }
    }

    #[test]
    fn 去掉中间一段留下前后两段() {
        let parts = segments(range(5.0, 10.0), 20.0);
        assert_eq!(parts, vec![range(0.0, 5.0), range(10.0, 20.0)]);
    }

    #[test]
    fn 从头删到底只剩一段() {
        assert_eq!(segments(range(0.0, 5.0), 20.0), vec![range(5.0, 20.0)]);
        assert_eq!(segments(range(15.0, 20.0), 20.0), vec![range(0.0, 15.0)]);
    }

    #[test]
    fn 覆盖整条视频时没有可留下的段落() {
        assert!(segments(range(0.0, 20.0), 20.0).is_empty());
        // 误差在半帧以内也不算。
        assert!(segments(range(0.0, 20.02), 20.0).is_empty());
    }

    #[test]
    fn 分割在切点处一分为二() {
        assert_eq!(
            split_segments(range(8.0, 12.0), 20.0),
            vec![range(0.0, 8.0), range(8.0, 20.0)]
        );
    }

    #[test]
    fn 切点贴着两端时不分() {
        assert!(split_segments(range(0.0, 5.0), 20.0).is_empty());
        assert!(split_segments(range(20.0, 20.0), 20.0).is_empty());
    }
}
