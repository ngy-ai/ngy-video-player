//! 视频裁切：把 `[开始, 结束]` 区间精确重编码成新文件。
//!
//! 两端都可以留空：开始留空表示从头，结束留空表示到最后。两端都留空时没有可裁的
//! 区间，界面会拒绝执行——这里不是「复制整份文件」的入口。
//!
//! 走重编码而不是流复制：流复制只能从关键帧开始，起点会偏前，而用户输入的时间是
//! 精确到毫秒的。代价是慢，所以任务放在后台线程，并用 ffmpeg 的 `-progress` 回报
//! 进度；取消时直接杀掉子进程并清掉半成品。

use anyhow::{Context as _, Result};
use futures::channel::mpsc::UnboundedSender;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 裁切输出的容器。
///
/// 选 mkv 而不是 mp4：mkv 能原样装下任意来源的音频与字幕轨，换成 mp4 就要为
/// PGS/ASS 之类找替身编码，失败面大得多。
pub const OUTPUT_EXTENSION: &str = "mkv";

/// 两端时间输入框允许的最短区间。低于这个值的请求基本是误操作，也让进度条失去意义。
const MIN_DURATION: f64 = 0.05;

/// 输入时间文本的解析结果。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParsedTime {
    /// 留空：开始表示「从头」，结束表示「到最后」。
    Empty,
    Value(f64),
    Invalid,
}

/// 解析 `90`、`1:30`、`01:02:03.500` 三种写法。
///
/// 冒号分段只有最后一段可以带小数；前面的段必须小于 60，避免 `1:75` 这种打错的
/// 写法被悄悄理解成 135 秒。
pub fn parse_time(text: &str) -> ParsedTime {
    let text = text.trim();
    if text.is_empty() {
        return ParsedTime::Empty;
    }

    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() > 3 {
        return ParsedTime::Invalid;
    }

    let mut total = 0.0;
    for (index, part) in parts.iter().enumerate() {
        let last = index + 1 == parts.len();
        if !digits_only(part, last) {
            return ParsedTime::Invalid;
        }
        let value: f64 = match part.parse() {
            Ok(value) => value,
            Err(_) => return ParsedTime::Invalid,
        };
        // 带冒号时最后一段也是「秒」，必须小于 60；只有单段写法才允许 `90` 这种
        // 「直接给秒数」的简写。
        if (!last || parts.len() > 1) && value >= 60.0 {
            return ParsedTime::Invalid;
        }
        total = total * 60.0 + value;
    }

    if total.is_finite() && total >= 0.0 {
        ParsedTime::Value(total)
    } else {
        ParsedTime::Invalid
    }
}

/// 只接受十进制数字，最后一段允许一个小数点（且不能以点结尾）。
fn digits_only(part: &str, allow_dot: bool) -> bool {
    if part.is_empty() || part.ends_with('.') {
        return false;
    }
    let mut dots = 0;
    let mut digits = 0;
    for ch in part.chars() {
        match ch {
            '0'..='9' => digits += 1,
            '.' if allow_dot => {
                dots += 1;
                if dots > 1 {
                    return false;
                }
            }
            _ => return false,
        }
    }
    digits > 0
}

/// 把秒数写成 `HH:MM:SS.mmm`，对话框里显示用。
pub fn format_time(seconds: f64) -> String {
    let seconds = seconds.max(0.0);
    let hours = (seconds / 3600.0).floor();
    let minutes = ((seconds - hours * 3600.0) / 60.0).floor();
    let rest = seconds - hours * 3600.0 - minutes * 60.0;
    format!("{hours:02.0}:{minutes:02.0}:{rest:06.3}")
}

/// 已换算成秒的裁切区间。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipRange {
    pub start: f64,
    pub end: f64,
}

impl ClipRange {
    pub fn duration(self) -> f64 {
        self.end - self.start
    }
}

/// 区间校验失败的原因。文案由这里给出，界面只负责显示。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeError {
    /// 两端都留空：没有可裁的区间。
    NothingToTrim,
    InvalidStart,
    InvalidEnd,
    /// 结束不晚于开始。
    StartNotBeforeEnd,
    /// 超出视频时长，或时长还没读出来。
    OutOfRange,
}

impl RangeError {
    pub fn message(self) -> &'static str {
        match self {
            RangeError::NothingToTrim => "开始和结束都留空，没有可裁切的区间",
            RangeError::InvalidStart => "开始时间格式不对，可用 90、1:30、01:02:03.5",
            RangeError::InvalidEnd => "结束时间格式不对，可用 90、1:30、01:02:03.5",
            RangeError::StartNotBeforeEnd => "结束时间必须晚于开始时间",
            RangeError::OutOfRange => "时间超出视频时长",
        }
    }
}

/// 把两段输入换算成裁切区间。
///
/// `Ok(None)` 表示两端都留空——调用方该禁用「开始裁切」，而不是复制整份文件。
pub fn range_from_inputs(
    start_text: &str,
    end_text: &str,
    duration: f64,
) -> Result<Option<ClipRange>, RangeError> {
    let start = parse_time(start_text);
    let end = parse_time(end_text);

    if start == ParsedTime::Empty && end == ParsedTime::Empty {
        return Ok(None);
    }
    if start == ParsedTime::Invalid {
        return Err(RangeError::InvalidStart);
    }
    if end == ParsedTime::Invalid {
        return Err(RangeError::InvalidEnd);
    }
    if duration <= 0.0 {
        return Err(RangeError::OutOfRange);
    }

    let start = match start {
        ParsedTime::Value(value) => value,
        _ => 0.0,
    };
    // 结束留空就是「到最后」；比时长多出几十毫秒只是浮点误差，不当成越界。
    let end = match end {
        ParsedTime::Value(value) => value,
        _ => duration,
    };

    if end > duration + 0.05 {
        return Err(RangeError::OutOfRange);
    }
    let end = end.min(duration);
    if end - start < MIN_DURATION {
        return Err(RangeError::StartNotBeforeEnd);
    }
    Ok(Some(ClipRange { start, end }))
}

/// 裁切结果的落点：与原文件同目录，名字加「 - 片段」，已存在就往后编号。
///
/// 不覆盖已有文件：裁切是个耗时操作，一次手滑把同名文件盖掉就找不回来了。
pub fn output_path(input: &Path) -> PathBuf {
    let dir = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "片段".to_string());
    let base = format!("{stem} - 片段");

    let mut candidate = dir.join(format!("{base}.{OUTPUT_EXTENSION}"));
    let mut index = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{base} ({index}).{OUTPUT_EXTENSION}"));
        index += 1;
        // 理论上不会走到这里；只是防止某个坏掉的目录让循环停不下来。
        if index > 999 {
            break;
        }
    }
    candidate
}

/// 一次裁切请求。
#[derive(Clone, Debug)]
pub struct ClipRequest {
    pub input: PathBuf,
    pub output: PathBuf,
    pub range: ClipRange,
}

/// 后台任务回报给界面的事件。
#[derive(Clone, Debug)]
pub enum ClipEvent {
    /// 0.0–1.0。
    Progress(f32),
    Done(ClipOutcome),
}

#[derive(Clone, Debug)]
pub enum ClipOutcome {
    Finished(PathBuf),
    Cancelled,
    Failed(String),
}

/// 正在跑的 ffmpeg 子进程。
///
/// 界面只持有一个句柄：进度通过事件通道回来，取消直接杀进程。
pub struct ClipJob {
    child: Arc<Mutex<Option<Child>>>,
    cancelled: Arc<AtomicBool>,
}

impl ClipJob {
    /// 启动后台裁切。命令拼装或进程启动失败会立即返回错误，界面不必等事件。
    pub fn start(request: ClipRequest, tx: UnboundedSender<ClipEvent>) -> Result<Self> {
        let executable = crate::platform::ffmpeg::executable()?;
        let mut child = build_command(&executable, &request)
            .spawn()
            .with_context(|| format!("启动 ffmpeg 失败：{}", executable.display()))?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let child = Arc::new(Mutex::new(Some(child)));
        let cancelled = Arc::new(AtomicBool::new(false));

        // stderr 必须持续读走：管道塞满会让 ffmpeg 卡在写日志上。只留最后几行，
        // 失败时这就是给用户看的原因。
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
        let total = request.range.duration();
        std::thread::spawn(move || {
            if let Some(stdout) = stdout {
                read_progress(stdout, total, &tx);
            }
            let status = crate::platform::ffmpeg::wait_polling(&task_child);
            let cancelled = task_cancelled.load(Ordering::SeqCst);

            let outcome = if cancelled {
                // 半成品留着只会让用户以为裁切成功了。
                let _ = std::fs::remove_file(&output);
                ClipOutcome::Cancelled
            } else {
                match status {
                    Some(Ok(status)) if status.success() => ClipOutcome::Finished(output),
                    Some(Ok(status)) => {
                        let detail = tail
                            .lock()
                            .unwrap()
                            .iter()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("\n");
                        ClipOutcome::Failed(if detail.is_empty() {
                            format!("ffmpeg 异常退出（{:?}）", status.code())
                        } else {
                            format!("ffmpeg 异常退出（{:?}）\n{detail}", status.code())
                        })
                    }
                    Some(Err(error)) => {
                        ClipOutcome::Failed(format!("等待 ffmpeg 结束失败：{error}"))
                    }
                    None => ClipOutcome::Failed("ffmpeg 进程已经结束".to_string()),
                }
            };
            let _ = tx.unbounded_send(ClipEvent::Done(outcome));
        });

        Ok(Self { child, cancelled })
    }

    /// 取消裁切。子进程会在几百毫秒内被回收，任务线程负责删掉半成品。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
    }
}

impl Drop for ClipJob {
    fn drop(&mut self) {
        // 关窗口退出时不能把 ffmpeg 留在后台继续写文件。
        self.cancel();
    }
}

/// 从 `-progress pipe:1` 的输出里读进度。
fn read_progress(stdout: impl Read, total: f64, tx: &UnboundedSender<ClipEvent>) {
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
        let _ = tx.unbounded_send(ClipEvent::Progress(progress));
    }
}

/// 拼 ffmpeg 参数。
///
/// `-ss` 放在 `-i` 前：ffmpeg 会先跳到目标附近再精确解码到目标帧，重编码时起点
/// 精确到帧，长视频也不用从第 0 秒解起。输出统一成 H.264 8bit + AAC：
/// 兼容性优先，HDR/10bit 源会被转成 SDR——这是「精确重编码」必须付出的代价。
fn build_command(executable: &Path, request: &ClipRequest) -> Command {
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
        .arg("-ss")
        .arg(format!("{:.3}", request.range.start))
        .arg("-i")
        .arg(&request.input)
        .arg("-t")
        .arg(format!("{:.3}", request.range.duration()))
        // 大写 V 排除封面图（attached_pic），否则第一路「视频」可能是一张 jpeg。
        .arg("-map")
        .arg("0:V:0?")
        .arg("-map")
        .arg("0:a?")
        .arg("-map")
        .arg("0:s?")
        .arg("-map_metadata")
        .arg("0")
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
        .arg("-c:s")
        .arg("copy")
        .arg("-avoid_negative_ts")
        .arg("make_zero")
        .arg(&request.output)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 时间支持三种写法() {
        assert_eq!(parse_time("90"), ParsedTime::Value(90.0));
        assert_eq!(parse_time("1:30"), ParsedTime::Value(90.0));
        assert_eq!(parse_time("01:02:03.500"), ParsedTime::Value(3723.5));
        assert_eq!(parse_time(" 1:30 "), ParsedTime::Value(90.0));
        assert_eq!(parse_time("0.5"), ParsedTime::Value(0.5));
    }

    #[test]
    fn 留空是特殊值() {
        assert_eq!(parse_time(""), ParsedTime::Empty);
        assert_eq!(parse_time("   "), ParsedTime::Empty);
    }

    #[test]
    fn 认不出的时间不会被猜成数字() {
        for bad in [
            "abc", "1:", ":30", "1:2:3:4", "1:75", "-5", "1.2.3", "1:2.", ".", "1e3",
        ] {
            assert_eq!(parse_time(bad), ParsedTime::Invalid, "{bad}");
        }
    }

    #[test]
    fn 两端都留空表示不裁切() {
        assert_eq!(range_from_inputs("", "", 600.0), Ok(None));
        assert_eq!(range_from_inputs("  ", "", 600.0), Ok(None));
    }

    #[test]
    fn 只填一端时另一端取边界() {
        assert_eq!(
            range_from_inputs("", "90", 600.0),
            Ok(Some(ClipRange {
                start: 0.0,
                end: 90.0
            }))
        );
        assert_eq!(
            range_from_inputs("90", "", 600.0),
            Ok(Some(ClipRange {
                start: 90.0,
                end: 600.0
            }))
        );
        assert_eq!(
            range_from_inputs("1:30", "2:00", 600.0),
            Ok(Some(ClipRange {
                start: 90.0,
                end: 120.0
            }))
        );
    }

    #[test]
    fn 越界与倒序被拒绝() {
        assert_eq!(
            range_from_inputs("", "700", 600.0),
            Err(RangeError::OutOfRange)
        );
        assert_eq!(
            range_from_inputs("90", "80", 600.0),
            Err(RangeError::StartNotBeforeEnd)
        );
        assert_eq!(
            range_from_inputs("90", "90", 600.0),
            Err(RangeError::StartNotBeforeEnd)
        );
        assert_eq!(
            range_from_inputs("abc", "", 600.0),
            Err(RangeError::InvalidStart)
        );
        assert_eq!(
            range_from_inputs("", "abc", 600.0),
            Err(RangeError::InvalidEnd)
        );
        // 时长还没读出来时不能裁切。
        assert_eq!(
            range_from_inputs("0", "10", 0.0),
            Err(RangeError::OutOfRange)
        );
    }

    #[test]
    fn 输出文件不覆盖已有文件() {
        let dir = std::env::temp_dir().join(format!("ngy-clip-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("课程 01.mp4");
        std::fs::write(&input, b"x").unwrap();

        let first = output_path(&input);
        assert_eq!(first, dir.join("课程 01 - 片段.mkv"));
        std::fs::write(&first, b"x").unwrap();

        let second = output_path(&input);
        assert_eq!(second, dir.join("课程 01 - 片段 (2).mkv"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn 时长格式化补零到毫秒() {
        assert_eq!(format_time(0.0), "00:00:00.000");
        assert_eq!(format_time(90.5), "00:01:30.500");
        assert_eq!(format_time(3723.25), "01:02:03.250");
    }
}
