//! 视频裁切：把 `[开始, 结束]` 区间导出成新文件。
//!
//! 两端都可以留空：开始留空表示从头，结束留空表示到最后。两端都留空时没有可裁的
//! 区间，界面会拒绝执行——这里不是「复制整份文件」的入口。
//!
//! 两条导出路径：重编码的切点精确到帧但慢；流复制直接搬码流、几秒完成，但起点
//! 必须落在关键帧上，所以要先探一次关键帧再替换掉用户给的秒数（见 `keyframe`）。
//! 两条路都在后台线程跑，用 ffmpeg 的 `-progress` 回报进度；取消时直接杀掉子进程
//! 并清掉半成品。

use anyhow::{Context as _, Result};
use futures::channel::mpsc::UnboundedSender;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

mod cutout;
mod encoder;
mod export;
mod keyframe;
mod loudness;

pub use export::{
    ColorMatrix, ColorRange, Crop, DEFAULT_BITRATE_KBPS, Edge, EncoderMode, ExportMode,
    ExportOptions, KeyframeAlign, OutputFormat, Pad, PixelFormat, Quality, RateControl, Rotate,
    Scale, VideoCodec, VideoTransform,
};
pub use loudness::Loudness;

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
/// HLS 只检查播放列表，同名分片交给 ffmpeg 覆盖——它们属于这一个播放列表。
pub fn output_path(input: &Path, format: OutputFormat) -> PathBuf {
    numbered_output(input, format, "片段")
}

/// 「删除中间段」的落点：后缀换成「删除片段」，免得和普通裁切混在一起。
pub fn cut_out_output_path(input: &Path) -> PathBuf {
    numbered_output(input, OutputFormat::Mkv, "删除片段")
}

fn numbered_output(input: &Path, format: OutputFormat, suffix: &str) -> PathBuf {
    let dir = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "片段".to_string());
    let base = format!("{stem} - {suffix}");
    let extension = format.extension();

    let mut candidate = dir.join(format!("{base}.{extension}"));
    let mut index = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{base} ({index}).{extension}"));
        index += 1;
        // 理论上不会走到这里；只是防止某个坏掉的目录让循环停不下来。
        if index > 999 {
            break;
        }
    }
    candidate
}

/// 要烧进画面的内置字幕轨。
///
/// 只有文本字幕能烧：点阵字幕（PGS / VobSub）是图片，`subtitles` 滤镜没有文本
/// 可画，只能继续作为软字幕保留。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BurnSubtitle {
    /// 容器里的第几条字幕流，对应 `-map 0:s:N`。
    pub stream: usize,
    /// 给界面显示的轨道名。
    pub name: String,
}

/// 一次裁切请求。
#[derive(Clone, Debug)]
pub struct ClipRequest {
    pub input: PathBuf,
    pub output: PathBuf,
    pub range: ClipRange,
    pub options: ExportOptions,
    /// 烧录哪一条内置字幕；`None` 表示不烧。
    pub burn: Option<BurnSubtitle>,
    /// 两遍响度的第一遍结果。界面不填，任务在启动时测出来写进去；
    /// `None` 表示走单遍 loudnorm。
    pub loudness: Option<Loudness>,
    /// 「删除中间段」：把 `range` 去掉，保留前后两段拼成一个文件。
    ///
    /// 这条路一定重编码：第二段从区间末尾起头，流复制找不到可用的关键帧，
    /// 拼出来的接缝会花屏。所以调用方传进来的 `mode` 会被忽略。
    pub remove_middle: bool,
    /// 源文件总时长。只有「删除中间段」需要——后半段要一直留到片尾。
    pub duration: f64,
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
    /// 产物可能是多个（分割会写出两个文件），按写出顺序排列。
    Finished(Vec<PathBuf>),
    Cancelled,
    Failed(String),
}

/// 删掉半成品。HLS 的输出是「播放列表 + 一堆分片」，只删 m3u8 会留下垃圾。
fn remove_output(output: &Path) {
    let _ = std::fs::remove_file(output);
    if output.extension().and_then(|ext| ext.to_str()) != Some("m3u8") {
        return;
    }
    let Some(dir) = output.parent() else {
        return;
    };
    let Some(stem) = output
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
    else {
        return;
    };
    let prefix = format!("{stem}_");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(&prefix) && name.ends_with(".ts") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// 流复制前把起点对到关键帧。
///
/// 返回调整后的请求与实际起点（没调整时为 `None`）。
///
/// 不对齐的后果不只是「多几秒」：ffmpeg 会从目标时间之前的关键帧开始复制，
/// 再把关键帧到目标时间之间的包全部丢掉，于是输出的首帧成了没有参考画面的
/// P 帧，播放器要花屏到下一个关键帧。所以这一步是正确性要求，不是体验优化。
///
/// 探测失败不阻断导出：按用户设定的时间切，至少还能拿到文件，只是开头可能坏。
fn align_to_keyframe(executable: &Path, mut request: ClipRequest) -> (ClipRequest, Option<f64>) {
    if request.options.mode != ExportMode::Copy {
        return (request, None);
    }
    let wanted = request.range;

    let frames = match keyframe::around(executable, &request.input, wanted.start) {
        Ok(frames) => frames,
        Err(error) => {
            log_warn!("探测关键帧失败，按设定时间流复制：{error:#}");
            return (request, None);
        }
    };

    let mut start = frames.pick(request.options.keyframe_align);
    // 向后对齐时下一个关键帧可能已经越过终点，这时退回前一个，
    // 而不是切出一个空区间。
    if start >= wanted.end - MIN_DURATION {
        start = frames.prev;
    }
    // 整段比一个 GOP 还短：两侧都对不上，原样切。
    if start >= wanted.end - MIN_DURATION {
        return (request, None);
    }
    // 输入本来就落在关键帧上：没变化，不必打扰用户。
    if (start - wanted.start).abs() < 0.0005 {
        return (request, None);
    }

    request.range = ClipRange {
        start,
        end: wanted.end,
    };
    (request, Some(start))
}

/// 正在跑的 ffmpeg 子进程。
///
/// 界面只持有一个句柄：进度通过事件通道回来，取消直接杀进程。
pub struct ClipJob {
    child: Arc<Mutex<Option<Child>>>,
    cancelled: Arc<AtomicBool>,
    /// 流复制吸附到关键帧之后的实际起点。与设定值相同时为 `None`，
    /// 界面据此决定要不要告诉用户「实际从哪里开始」。
    pub aligned: Option<f64>,
}

impl ClipJob {
    /// 启动后台裁切。命令拼装或进程启动失败会立即返回错误，界面不必等事件。
    pub fn start(mut request: ClipRequest, tx: UnboundedSender<ClipEvent>) -> Result<Self> {
        let executable = crate::platform::ffmpeg::executable()?;
        // 编码器在任务线程里探测：`ffmpeg -encoders` 要起一个进程，不该卡界面。
        let encoder = encoder::pick(request.options.codec, request.options.encoder_mode)
            .map_err(anyhow::Error::msg)?;
        // 「删除中间段」走另一条路：先分别编码前后两段，再拼起来。它不看流复制
        // 的关键帧对齐（区间是「要删掉的部分」，对齐会改变用户选的切点）。
        if request.remove_middle {
            request.options.mode = ExportMode::Exact;
            // 两遍响度在这里不划算：每段都要单独量一遍，收益抵不上时间。
            request.loudness = None;
            let burn = match &request.burn {
                Some(subtitle) => Some(extract_subtitle(&executable, &request.input, subtitle)?),
                None => None,
            };
            return cutout::start(&executable, request, encoder, burn, tx);
        }
        // 流复制只能在关键帧上起步，探测放在拼命令之前。
        let (mut request, aligned) = align_to_keyframe(&executable, request);
        // 两遍响度：先空跑一遍量出整段的响度。测量失败就退回单遍——多花的时间
        // 已经花了，不该因为「量不准」让整单导出失败。
        if request.options.wants_two_pass_loudness() {
            match measure_loudness(&executable, &request) {
                Ok(loudness) => request.loudness = Some(loudness),
                Err(error) => log_warn!("响度测量失败，退回单遍标准化：{error:#}"),
            }
        }
        // 烧录要先把那条字幕抽成独立的 ass：`subtitles` 滤镜只认文件路径，
        // 读不到容器里的流。抽不出来的话整单没法做，直接返回错误让界面显示原因。
        let burn = match &request.burn {
            Some(subtitle) => Some(extract_subtitle(&executable, &request.input, subtitle)?),
            None => None,
        };
        let mut child = export::build_command(&executable, &request, encoder, burn.as_deref())
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
        let task_burn = burn;
        std::thread::spawn(move || {
            if let Some(stdout) = stdout {
                read_progress(stdout, total, &tx);
            }
            let status = crate::platform::ffmpeg::wait_polling(&task_child);
            let cancelled = task_cancelled.load(Ordering::SeqCst);

            let outcome = if cancelled {
                // 半成品留着只会让用户以为裁切成功了。HLS 还要清掉分片。
                remove_output(&output);
                ClipOutcome::Cancelled
            } else {
                match status {
                    Some(Ok(status)) if status.success() => ClipOutcome::Finished(vec![output]),
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
            // 临时字幕文件是这次任务的中间产物：ffmpeg 已经退出，留着只会占地方。
            if let Some(path) = &task_burn {
                let _ = std::fs::remove_file(path);
            }
            let _ = tx.unbounded_send(ClipEvent::Done(outcome));
        });

        Ok(Self {
            child,
            cancelled,
            aligned,
        })
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

/// 两遍响度的第一遍：只解码音频，让 loudnorm 把测量值打到 stderr 上。
///
/// 用 `-f null -` 丢弃输出，除了解码没有别的开销；没有音轨时会失败，调用方退回单遍。
fn measure_loudness(executable: &Path, request: &ClipRequest) -> Result<Loudness> {
    let output = Command::new(executable)
        .arg("-hide_banner")
        .arg("-nostdin")
        .arg("-nostats")
        // loudnorm 的统计走 info 级别的日志；压到 error 就什么都拿不到了。
        .arg("-loglevel")
        .arg("info")
        .arg("-ss")
        .arg(format!("{:.3}", request.range.start))
        .arg("-i")
        .arg(&request.input)
        .arg("-t")
        .arg(format!("{:.3}", request.range.duration()))
        .arg("-vn")
        .arg("-sn")
        .arg("-dn")
        .arg("-af")
        .arg(format!("loudnorm={}", loudness::TARGETS))
        .arg("-f")
        .arg("null")
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("启动 ffmpeg 测量响度失败：{}", executable.display()))?;
    let text = String::from_utf8_lossy(&output.stderr);
    loudness::parse(&text).ok_or_else(|| anyhow::Error::msg("ffmpeg 没有输出响度统计"))
}

/// 把容器里的第 `stream` 条字幕抽成临时 ass 文件。
///
/// 统一转 ass：它是文本字幕里表达力最强的格式，srt 那点样式转过去不会丢；
/// 反过来 ass 转 srt 会丢掉定位与字体信息。
fn extract_subtitle(executable: &Path, input: &Path, subtitle: &BurnSubtitle) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "ngy-burn-{}-{}.ass",
        std::process::id(),
        subtitle.stream
    ));
    let output = Command::new(executable)
        .arg("-hide_banner")
        .arg("-nostdin")
        .arg("-y")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(input)
        .arg("-map")
        .arg(format!("0:s:{}", subtitle.stream))
        .arg("-c:s")
        .arg("ass")
        .arg("-f")
        .arg("ass")
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("启动 ffmpeg 抽取字幕失败：{}", executable.display()))?;
    if output.status.success() {
        return Ok(path);
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail.trim();
    // 点阵字幕走到这里：ffmpeg 会说「无法把 pgs 转成 ass」。这句话对用户太抽象，
    // 所以前半句先把可能的原因说清楚。
    let mut message = format!(
        "抽取「{}」失败：这条轨道可能不是文本字幕，点阵字幕（PGS/VobSub）只能保留为软字幕",
        subtitle.name
    );
    if !detail.is_empty() {
        message.push('\n');
        message.push_str(detail);
    }
    Err(anyhow::Error::msg(message))
}

/// 从 `-progress pipe:1` 的输出里读进度。
fn read_progress(stdout: impl Read, total: f64, tx: &UnboundedSender<ClipEvent>) {
    read_progress_mapped(stdout, total, |fraction| fraction, tx)
}

/// 读进度并换算成整单的比例。多段拼接时每段只占一截，`map` 负责把它挪到
/// 对应的位置上。
fn read_progress_mapped(
    stdout: impl Read,
    total: f64,
    map: impl Fn(f32) -> f32,
    tx: &UnboundedSender<ClipEvent>,
) {
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
        let _ = tx.unbounded_send(ClipEvent::Progress(map(progress)));
    }
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

        let first = output_path(&input, OutputFormat::Mkv);
        assert_eq!(first, dir.join("课程 01 - 片段.mkv"));
        std::fs::write(&first, b"x").unwrap();

        let second = output_path(&input, OutputFormat::Mkv);
        assert_eq!(second, dir.join("课程 01 - 片段 (2).mkv"));

        // HLS 只认自己的播放列表，不会被已有的 mkv 影响。
        let hls = output_path(&input, OutputFormat::Hls);
        assert_eq!(hls, dir.join("课程 01 - 片段.m3u8"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn 时长格式化补零到毫秒() {
        assert_eq!(format_time(0.0), "00:00:00.000");
        assert_eq!(format_time(90.5), "00:01:30.500");
        assert_eq!(format_time(3723.25), "01:02:03.250");
    }
}
