//! 单文件的裁切任务：拼命令、读进度、收尾清理。
//!
//! 两条导出路径：重编码的切点精确到帧但慢；流复制直接搬码流、几秒完成，但起点
//! 必须落在关键帧上，所以要先探一次关键帧再替换掉用户给的秒数（见 `keyframe`）。
//! 两条路都在后台线程跑，用 ffmpeg 的 `-progress` 回报进度；取消时直接杀掉子进程
//! 并清掉半成品。

use super::encoder;
use super::export::{self, ExportMode};
use super::keyframe;
use super::loudness::{self, Loudness};
use super::range::MIN_DURATION;
use super::{BurnSubtitle, ClipEvent, ClipOutcome, ClipRange, ClipRequest, CutKind};
use anyhow::{Context as _, Result, anyhow};
use futures::channel::mpsc::UnboundedSender;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 把临时文件按目标大小切成若干段。
///
/// 段时长 = 目标字节数 ÷ 实际码率，实际码率由「文件大小 ÷ 时长」量出来：CRF、
/// 滤镜、响度都会影响码率，只有量最终文件才准。切开是流复制，切点落在关键帧上，
/// 所以每段会比目标略小一点。
fn split_file(
    executable: &Path,
    source: &Path,
    output: &Path,
    split_mb: u32,
    duration: f64,
) -> Result<Vec<PathBuf>> {
    let bytes = std::fs::metadata(source)
        .with_context(|| format!("读取临时文件失败：{}", source.display()))?
        .len() as f64;
    if bytes <= 0.0 || duration <= 0.0 {
        return Err(anyhow!("临时文件是空的，没法切分"));
    }
    let target = split_mb.max(1) as f64 * 1024.0 * 1024.0;
    let bytes_per_second = (bytes / duration).max(1.0);
    let segment_seconds = (target / bytes_per_second).max(1.0);
    let pattern = export::split_pattern(output);

    let completed = Command::new(executable)
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-i")
        .arg(source)
        .arg("-map")
        .arg("0")
        .arg("-c")
        .arg("copy")
        .arg("-map_chapters")
        .arg("0")
        .arg("-f")
        .arg("segment")
        .arg("-segment_time")
        .arg(format!("{segment_seconds:.3}"))
        // 每段都从 0 开始计时，单独拿一段出来也能正常播。
        .arg("-reset_timestamps")
        .arg("1")
        .arg(&pattern)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("启动切分失败：{}", executable.display()))?;
    if !completed.status.success() {
        let detail = String::from_utf8_lossy(&completed.stderr);
        return Err(anyhow!("切分失败：{}", detail.trim()));
    }

    let files = collect_split_outputs(output);
    if files.is_empty() {
        return Err(anyhow!("切分没有产出任何文件：{}", pattern.display()));
    }
    let _ = std::fs::remove_file(source);
    Ok(files)
}

/// 按大小切分写出来的那些段：`{stem}_001.mkv`…，按名字排序就是播放顺序。
fn collect_split_outputs(output: &Path) -> Vec<PathBuf> {
    let Some(dir) = output.parent() else {
        return Vec::new();
    };
    let Some(stem) = output
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
    else {
        return Vec::new();
    };
    let prefix = format!("{stem}_");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut segments: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().and_then(|ext| ext.to_str()) == Some("mkv")
                && path
                    .file_name()
                    .map(|name| name.to_string_lossy().starts_with(&prefix))
                    .unwrap_or(false)
        })
        .collect();
    segments.sort();
    segments
}

fn remove_split_outputs(output: &Path) {
    for segment in collect_split_outputs(output) {
        let _ = std::fs::remove_file(segment);
    }
}

/// 删掉半成品。切片输出的产物是「播放列表 + 初始化段 + 一堆分片」，
/// 只删 m3u8/mpd 会留下一地垃圾。
///
/// 分片名统一带播放列表的 stem 前缀（见 `clip::export`），所以按前缀就能认全：
/// `.ts` 是 HLS 的 TS 分片，`.m4s` 是 fMP4 与 DASH 分片，`.mp4` 是 fMP4 的初始化段。
pub(super) fn remove_output(output: &Path) {
    let _ = std::fs::remove_file(output);
    let extension = output.extension().and_then(|ext| ext.to_str());
    if !matches!(extension, Some("m3u8" | "mpd")) {
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
        let segment = name.ends_with(".ts") || name.ends_with(".m4s") || name.ends_with(".mp4");
        if name.starts_with(&prefix) && segment {
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
    // 起点就在文件开头：整段导出（批处理）不必再起一个进程去探关键帧。
    if wanted.start <= 0.0005 {
        return (request, None);
    }

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
    /// 由多段任务组装：它们自己管进程，只借用同一套取消与半成品清理。
    pub(super) fn new(
        child: Arc<Mutex<Option<Child>>>,
        cancelled: Arc<AtomicBool>,
        aligned: Option<f64>,
    ) -> Self {
        Self {
            child,
            cancelled,
            aligned,
        }
    }

    /// 启动后台裁切。命令拼装或进程启动失败会立即返回错误，界面不必等事件。
    pub fn start(mut request: ClipRequest, tx: UnboundedSender<ClipEvent>) -> Result<Self> {
        let executable = crate::platform::ffmpeg::executable()?;
        // 编码器在任务线程里探测：`ffmpeg -encoders` 要起一个进程，不该卡界面。
        let encoder = encoder::pick(request.options.codec, request.options.encoder_mode)
            .map_err(anyhow::Error::msg)?;
        // 「删除中间段」走另一条路：先分别编码前后两段，再拼起来。它不看流复制
        // 的关键帧对齐（区间是「要删掉的部分」，对齐会改变用户选的切点）。
        if request.kind != CutKind::Keep {
            request.options.mode = ExportMode::Exact;
            // 两遍响度在这里不划算：每段都要单独量一遍，收益抵不上时间。
            request.loudness = None;
            let burn = match &request.burn {
                Some(subtitle) => Some(extract_subtitle(&executable, &request.input, subtitle)?),
                None => None,
            };
            return super::cutout::start(&executable, request, encoder, burn, tx);
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
        // 按大小切分要分两步走：ffmpeg 的 segment muxer 只按时间切，没有按大小的选项。
        // 先整段编码成临时文件，量出真实码率算出每段该多长，再流复制切开。
        let split = request.options.format.is_split();
        let final_output = request.output.clone();
        if split {
            request.output = export::split_temp_path(&final_output);
        }
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
        // 切分时 `output` 是临时文件，成片由第二遍切出来。
        let output = request.output.clone();
        let split_mb = request.options.split_mb;
        // 开头定格会多出几秒，进度分母不跟着加长，进度条会提前走到满。
        let total = request.options.output_duration(request.range.duration());
        let split_executable = executable.clone();
        let task_burn = burn;
        std::thread::spawn(move || {
            if let Some(stdout) = stdout {
                read_progress(stdout, total, &tx);
            }
            let status = crate::platform::ffmpeg::wait_polling(&task_child);
            let cancelled = task_cancelled.load(Ordering::SeqCst);

            let outcome = if cancelled {
                // 半成品留着只会让用户以为裁切成功了。切片输出还要清掉分片。
                remove_output(&output);
                if split {
                    remove_split_outputs(&final_output);
                }
                ClipOutcome::Cancelled
            } else {
                match status {
                    Some(Ok(status)) if status.success() => {
                        if split {
                            // 产物是一串带编号的 mkv，全部报回去，界面上「定位」
                            // 才有东西可指。
                            match split_file(
                                &split_executable,
                                &output,
                                &final_output,
                                split_mb,
                                total,
                            ) {
                                Ok(files) => ClipOutcome::Finished(files),
                                Err(message) => {
                                    let _ = std::fs::remove_file(&output);
                                    ClipOutcome::Failed(format!("{message:#}"))
                                }
                            }
                        } else {
                            ClipOutcome::Finished(vec![output.clone()])
                        }
                    }
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
            // 切分的第一遍失败时，那个 `.part.mkv` 也是半成品，不能留在目录里。
            if split && !matches!(outcome, ClipOutcome::Finished(_)) {
                let _ = std::fs::remove_file(&output);
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
pub(super) fn measure_loudness(executable: &Path, request: &ClipRequest) -> Result<Loudness> {
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
pub(super) fn read_progress_mapped(
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
