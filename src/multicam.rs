//! 双机位并排导出：把两个机位缩到同样大小，左右拼成一条 1280×720 的视频。
//!
//! 这不是「现场切机位」的多机位剪辑，而是把两路画面同时摆出来做粗剪 / 对照。
//! 机位对齐只做「右机位跳过开头」：双机位最常见的情况是 B 机早开，剪掉 B 的
//! 前几秒就能和 A 对上；反过来（B 晚开）把文件顺序换一下即可。这个偏移可以手填，
//! 也可以点「自动对齐」让 [`sync`] 听两路音频算出来。
//!
//! 两路统一缩放到 640×360 并补边，避免宽高比不同时被拉变形；帧率统一到 30，
//! 否则 `hstack` 会因为两路帧率不一致直接失败。

pub mod sync;

use anyhow::{Context as _, Result};
use futures::channel::mpsc::UnboundedSender;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 单格尺寸。两格拼起来是 1280×720。
const CELL_WIDTH: u32 = 640;
const CELL_HEIGHT: u32 = 360;
/// 输出统一帧率。
const OUTPUT_FPS: u32 = 30;
/// 单格时长上限，防止误填一个巨大的偏移去做无意义的解码。
const MAX_SKIP: f64 = 600.0;

/// 一次双机位导出请求。
#[derive(Clone, Debug)]
pub struct MulticamRequest {
    pub left: PathBuf,
    pub right: PathBuf,
    /// 右机位从第几秒开始取（对齐用）。
    pub right_skip: f64,
    /// 输出时长（秒），由调用方按两个机位的时长取交集。
    pub duration: f64,
    pub output: PathBuf,
}

#[derive(Clone, Debug)]
pub enum MulticamEvent {
    Progress(f32),
    Done(MulticamOutcome),
}

#[derive(Clone, Debug)]
pub enum MulticamOutcome {
    Finished(PathBuf),
    Cancelled,
    Failed(String),
}

pub struct MulticamJob {
    child: Arc<Mutex<Option<Child>>>,
    cancelled: Arc<AtomicBool>,
}

impl MulticamJob {
    pub fn start(request: MulticamRequest, tx: UnboundedSender<MulticamEvent>) -> Result<Self> {
        let executable = crate::platform::ffmpeg::executable()?;
        let mut child = build_command(&executable, &request)
            .spawn()
            .with_context(|| format!("启动 ffmpeg 失败：{}", executable.display()))?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let child = Arc::new(Mutex::new(Some(child)));
        let cancelled = Arc::new(AtomicBool::new(false));

        let tail = Arc::new(Mutex::new(VecDeque::new()));
        if let Some(stderr) = stderr {
            let tail = tail.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(|line| line.ok()) {
                    let mut tail = tail.lock().unwrap();
                    if tail.len() >= 12 {
                        tail.pop_front();
                    }
                    tail.push_back(line);
                }
            });
        }

        let task_child = child.clone();
        let task_cancelled = cancelled.clone();
        let output = request.output.clone();
        let total = request.duration;
        std::thread::spawn(move || {
            if let Some(stdout) = stdout {
                read_progress(stdout, total, &tx);
            }
            let status = crate::platform::ffmpeg::wait_polling(&task_child);
            let cancelled = task_cancelled.load(Ordering::SeqCst);
            let outcome = if cancelled {
                let _ = std::fs::remove_file(&output);
                MulticamOutcome::Cancelled
            } else {
                match status {
                    Some(Ok(status)) if status.success() => MulticamOutcome::Finished(output),
                    Some(Ok(status)) => {
                        let detail = tail
                            .lock()
                            .unwrap()
                            .iter()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("\n");
                        MulticamOutcome::Failed(if detail.is_empty() {
                            format!("ffmpeg 异常退出（{:?}）", status.code())
                        } else {
                            format!("ffmpeg 异常退出（{:?}）\n{detail}", status.code())
                        })
                    }
                    Some(Err(error)) => {
                        MulticamOutcome::Failed(format!("等待 ffmpeg 结束失败：{error}"))
                    }
                    None => MulticamOutcome::Failed("ffmpeg 进程已经结束".to_string()),
                }
            };
            let _ = tx.unbounded_send(MulticamEvent::Done(outcome));
        });

        Ok(Self { child, cancelled })
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
    }
}

impl Drop for MulticamJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// 双机位结果落点：`<左机位名> - 双机位.mp4`。
pub fn output_path(left: &Path) -> PathBuf {
    let dir = left.parent().unwrap_or_else(|| Path::new("."));
    let stem = left
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "双机位".to_string());
    let base = format!("{stem} - 双机位");

    let mut candidate = dir.join(format!("{base}.mp4"));
    let mut index = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{base} ({index}).mp4"));
        index += 1;
        if index > 999 {
            break;
        }
    }
    candidate
}

/// 用 `ffmpeg -i` 的 stderr 读时长。播放器只认识当前文件的时长，第二个机位
/// 要单独探一次；没有 ffprobe 也能工作。
pub fn probe_duration(input: &Path) -> Result<f64> {
    let executable = crate::platform::ffmpeg::executable()?;
    let output = Command::new(executable)
        .args(["-hide_banner", "-nostdin"])
        .arg("-i")
        .arg(input)
        .output()
        .with_context(|| format!("读取时长失败：{}", input.display()))?;
    // 只给 -i 不给输出时 ffmpeg 必然非零退出，真正的信息在 stderr 里。
    let text = String::from_utf8_lossy(&output.stderr);
    parse_duration(&text).ok_or_else(|| {
        anyhow::anyhow!(
            "读不出这个文件的时长：{}。换一个文件，或先用格式转换转成常规 mp4/mkv。",
            input.display()
        )
    })
}

/// 从 ffmpeg 的 `Duration: 00:01:23.45` 行解析秒数。
fn parse_duration(text: &str) -> Option<f64> {
    let line = text.lines().find(|line| line.contains("Duration:"))?;
    let value = line.split("Duration:").nth(1)?.trim();
    let value = value.split(',').next()?.trim();
    let mut parts = value.split(':');
    let hours: f64 = parts.next()?.parse().ok()?;
    let minutes: f64 = parts.next()?.parse().ok()?;
    let seconds: f64 = parts.next()?.parse().ok()?;
    let total = hours * 3600.0 + minutes * 60.0 + seconds;
    total.is_finite().then_some(total)
}

/// 两路取交集。右机位跳过开头后不足一秒就没法拼了。
pub fn overlap(left: f64, right: f64, right_skip: f64) -> Option<f64> {
    let right_left = right - right_skip.clamp(0.0, MAX_SKIP);
    let duration = left.min(right_left);
    (duration >= 0.5).then_some(duration)
}

fn filter_graph() -> String {
    let cell = |index: usize| {
        format!(
            "[{index}:v]scale={CELL_WIDTH}:{CELL_HEIGHT}:\
             force_original_aspect_ratio=decrease,\
             pad={CELL_WIDTH}:{CELL_HEIGHT}:(ow-iw)/2:(oh-ih)/2,\
             setsar=1,fps={OUTPUT_FPS},format=yuv420p[v{index}]"
        )
    };
    format!("{};{};[v0][v1]hstack=inputs=2[v]", cell(0), cell(1))
}

fn build_command(executable: &Path, request: &MulticamRequest) -> Command {
    let mut command = Command::new(executable);
    command
        .arg("-hide_banner")
        .arg("-nostdin")
        .arg("-y")
        .arg("-nostats")
        .arg("-loglevel")
        .arg("error")
        .arg("-progress")
        .arg("pipe:1")
        .arg("-i")
        .arg(&request.left)
        .arg("-ss")
        .arg(format!("{:.3}", request.right_skip.max(0.0)))
        .arg("-i")
        .arg(&request.right)
        .arg("-filter_complex")
        .arg(filter_graph())
        .arg("-map")
        .arg("[v]")
        // 只取左机位的声音：两路同时混会叠成回声，现场收音本来就该选一路。
        .arg("-map")
        .arg("0:a?")
        .arg("-t")
        .arg(format!("{:.3}", request.duration.max(0.0)))
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("medium")
        .arg("-crf")
        .arg("20")
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("192k")
        .arg("-movflags")
        .arg("+faststart")
        .arg(&request.output)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn read_progress(stdout: impl Read, total: f64, tx: &UnboundedSender<MulticamEvent>) {
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
        let _ = tx.unbounded_send(MulticamEvent::Progress(progress));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> MulticamRequest {
        MulticamRequest {
            left: PathBuf::from("a.mkv"),
            right: PathBuf::from("b.mkv"),
            right_skip: 2.5,
            duration: 30.0,
            output: PathBuf::from("out.mp4"),
        }
    }

    fn args() -> Vec<String> {
        build_command(Path::new("ffmpeg"), &request())
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn has(args: &[String], needle: &[&str]) -> bool {
        args.windows(needle.len()).any(|slice| {
            slice
                .iter()
                .zip(needle)
                .all(|(arg, want)| arg.as_str() == *want)
        })
    }

    #[test]
    fn 两路都缩放补边并统一帧率() {
        let graph = filter_graph();
        assert!(graph.contains("[0:v]scale=640:360"));
        assert!(graph.contains("[1:v]scale=640:360"));
        assert!(graph.contains("force_original_aspect_ratio=decrease"));
        assert!(graph.contains("pad=640:360"));
        assert!(graph.contains("fps=30"));
        assert!(graph.contains("[v0][v1]hstack=inputs=2[v]"));
    }

    #[test]
    fn 右机位跳过开头且只取左机位音频() {
        let args = args();
        assert!(has(&args, &["-ss", "2.500"]));
        assert!(has(&args, &["-map", "[v]"]));
        assert!(has(&args, &["-map", "0:a?"]));
        assert!(has(&args, &["-t", "30.000"]));
        // 右机位的音频不能映射，否则两路声音会叠在一起。
        assert!(!has(&args, &["-map", "1:a?"]));
    }

    #[test]
    fn 时长交集不足半秒时不导出() {
        assert_eq!(overlap(10.0, 30.0, 2.0), Some(10.0));
        assert_eq!(overlap(10.0, 12.0, 2.0), Some(10.0));
        assert_eq!(overlap(10.0, 12.0, 11.8), None);
        assert_eq!(overlap(10.0, 5.0, 0.0), Some(5.0));
    }

    #[test]
    fn 从_ffmpeg_输出里解析时长() {
        let text = "  Duration: 01:02:03.50, start: 0.000000, bitrate: 1234 kb/s";
        assert_eq!(parse_duration(text), Some(3723.5));
        assert_eq!(parse_duration("no duration here"), None);
    }

    #[test]
    fn 输出文件不覆盖已有文件() {
        let dir = std::env::temp_dir().join(format!("ngy-multicam-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let left = dir.join("课程 01.mkv");
        std::fs::write(&left, b"x").unwrap();

        let first = output_path(&left);
        assert_eq!(first, dir.join("课程 01 - 双机位.mp4"));
        std::fs::write(&first, b"x").unwrap();

        let second = output_path(&left);
        assert_eq!(second, dir.join("课程 01 - 双机位 (2).mp4"));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
