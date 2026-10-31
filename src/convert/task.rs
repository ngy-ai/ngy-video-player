//! 后台转换任务：逐个文件跑 ffmpeg，回报进度、可取消。
//!
//! 一个批次只有一个 ffmpeg 在跑：同时开几个编码器只会让每一个都变慢，还要多占
//! 几倍的磁盘写入。进度靠 `-progress pipe:1`，总时长从 ffmpeg 打开输入时打在
//! stderr 上的 `Duration:` 行里读——播放器这边没有 ffprobe，也不用为进度再探一次。

use anyhow::Result;
use futures::channel::mpsc::UnboundedSender;
use std::collections::{HashSet, VecDeque};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::TargetFormat;

/// 改名后的后缀。只在「目标文件已存在」时用上，说明这份是转出来的。
const SUFFIX: &str = " - 转换";
/// 编号上限：防止某个坏掉的目录让找名字的循环停不下来。
const MAX_INDEX: usize = 999;
/// 失败时留给用户看的 stderr 行数。
const TAIL_LINES: usize = 12;

/// 一次转换请求。
#[derive(Clone, Debug)]
pub struct ConvertRequest {
    pub input: PathBuf,
    pub output: PathBuf,
    pub target: TargetFormat,
}

/// 给一批输入配上输出路径。
///
/// 同一批次里两条请求不会落到同一个文件上：`a.mkv` 和 `a.mp4` 一起转 mp4 时，
/// 第二条会被编号成 `a - 转换 (2).mp4`，否则两路 ffmpeg 会互相踩掉对方的输出。
pub fn plan(inputs: &[PathBuf], target: TargetFormat) -> Vec<ConvertRequest> {
    let mut taken: HashSet<PathBuf> = HashSet::new();
    inputs
        .iter()
        .map(|input| {
            let output = output_path(input, target, &taken);
            taken.insert(output.clone());
            ConvertRequest {
                input: input.clone(),
                output,
                target,
            }
        })
        .collect()
}

/// 输出落点：优先用同名换后缀，撞上已有文件就加「 - 转换」再编号。
///
/// 绝不覆盖原文件：这条路径最常见的误操作就是把 `a.mp4` 转成 `a.mp4`，
/// 覆盖掉原片就找不回来了。
fn output_path(input: &Path, target: TargetFormat, taken: &HashSet<PathBuf>) -> PathBuf {
    let dir = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "转换".to_string());
    let extension = target.extension();

    let direct = dir.join(format!("{stem}.{extension}"));
    if !direct.exists() && !taken.contains(&direct) {
        return direct;
    }

    let base = format!("{stem}{SUFFIX}");
    let mut candidate = dir.join(format!("{base}.{extension}"));
    let mut index = 2;
    while candidate.exists() || taken.contains(&candidate) {
        candidate = dir.join(format!("{base} ({index}).{extension}"));
        index += 1;
        if index > MAX_INDEX {
            break;
        }
    }
    candidate
}

/// 后台任务回报给界面的事件。
#[derive(Clone, Debug)]
pub enum ConvertEvent {
    /// 开始处理第 `index` 项。
    ItemStarted {
        index: usize,
        input: PathBuf,
    },
    /// 当前项进度。总时长未知（直播流、坏文件）时 `ratio` 是 `None`。
    Progress {
        index: usize,
        ratio: Option<f32>,
    },
    ItemFinished {
        index: usize,
        output: PathBuf,
    },
    ItemFailed {
        index: usize,
        input: PathBuf,
        message: String,
    },
    /// 整个批次收尾，之后不会再有事件。
    Finished(ConvertOutcome),
}

#[derive(Clone, Debug)]
pub struct ConvertFailure {
    pub input: PathBuf,
    pub message: String,
}

/// 一个批次的结果。
#[derive(Clone, Debug, Default)]
pub struct ConvertSummary {
    pub outputs: Vec<PathBuf>,
    pub failures: Vec<ConvertFailure>,
    /// 取消后没处理的项数，含当时正在转的那一项。
    pub skipped: usize,
}

#[derive(Clone, Debug)]
pub enum ConvertOutcome {
    Done(ConvertSummary),
    Cancelled(ConvertSummary),
}

/// 正在跑的转换批次。界面只持有一个句柄：进度走事件通道，取消直接杀当前子进程。
pub struct ConvertJob {
    child: Arc<Mutex<Option<Child>>>,
    cancelled: Arc<AtomicBool>,
}

impl ConvertJob {
    /// 启动批次。第一个文件都还没开始就失败（找不到 ffmpeg、列表为空）时立即返回错误，
    /// 界面不必等一个不会来的事件。
    pub fn start(requests: Vec<ConvertRequest>, tx: UnboundedSender<ConvertEvent>) -> Result<Self> {
        if requests.is_empty() {
            anyhow::bail!("没有需要转换的文件");
        }
        let executable = crate::platform::ffmpeg::executable()?;
        let child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
        let cancelled = Arc::new(AtomicBool::new(false));

        let task_child = child.clone();
        let task_cancelled = cancelled.clone();
        std::thread::spawn(move || {
            let total = requests.len();
            let mut summary = ConvertSummary::default();
            let mut skipped = 0;

            for (index, request) in requests.into_iter().enumerate() {
                if task_cancelled.load(Ordering::SeqCst) {
                    skipped = total - index;
                    break;
                }
                let _ = tx.unbounded_send(ConvertEvent::ItemStarted {
                    index,
                    input: request.input.clone(),
                });

                match run_one(
                    &executable,
                    &request,
                    index,
                    &task_child,
                    &task_cancelled,
                    &tx,
                ) {
                    ItemResult::Ok => {
                        summary.outputs.push(request.output.clone());
                        let _ = tx.unbounded_send(ConvertEvent::ItemFinished {
                            index,
                            output: request.output,
                        });
                    }
                    ItemResult::Cancelled => {
                        skipped = total - index;
                        break;
                    }
                    ItemResult::Failed(message) => {
                        let _ = tx.unbounded_send(ConvertEvent::ItemFailed {
                            index,
                            input: request.input.clone(),
                            message: message.clone(),
                        });
                        summary.failures.push(ConvertFailure {
                            input: request.input,
                            message,
                        });
                    }
                }
            }
            summary.skipped = skipped;

            let outcome = if task_cancelled.load(Ordering::SeqCst) {
                ConvertOutcome::Cancelled(summary)
            } else {
                ConvertOutcome::Done(summary)
            };
            let _ = tx.unbounded_send(ConvertEvent::Finished(outcome));
        });

        Ok(Self { child, cancelled })
    }

    /// 取消。正在跑的 ffmpeg 会被杀掉，任务线程负责删掉半成品并回报结果。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
    }
}

impl Drop for ConvertJob {
    fn drop(&mut self) {
        // 关窗口退出时不能把 ffmpeg 留在后台继续写文件。
        self.cancel();
    }
}

enum ItemResult {
    Ok,
    Cancelled,
    Failed(String),
}

fn run_one(
    executable: &Path,
    request: &ConvertRequest,
    index: usize,
    slot: &Arc<Mutex<Option<Child>>>,
    cancelled: &Arc<AtomicBool>,
    tx: &UnboundedSender<ConvertEvent>,
) -> ItemResult {
    let mut command = build_command(executable, request);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return ItemResult::Failed(format!("启动 ffmpeg 失败：{error}")),
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    // 先进共享槽位再开始读：取消要能拿到它才杀得掉进程。
    *slot.lock().unwrap() = Some(child);

    let duration = Arc::new(Mutex::new(None));
    let tail = Arc::new(Mutex::new(VecDeque::new()));
    if let Some(stderr) = stderr {
        let tail = tail.clone();
        let duration = duration.clone();
        std::thread::spawn(move || read_stderr(stderr, tail, duration));
    }
    // 阻塞读到进程退出：取消时子进程被杀，管道关闭，这里自然返回。
    if let Some(stdout) = stdout {
        read_progress(stdout, &duration, index, tx);
    }

    let status = crate::platform::ffmpeg::wait_polling(slot);
    if cancelled.load(Ordering::SeqCst) {
        // 半成品留着只会让用户以为这一项转换成功了。
        let _ = std::fs::remove_file(&request.output);
        return ItemResult::Cancelled;
    }

    match status {
        Some(Ok(status)) if status.success() => ItemResult::Ok,
        Some(Ok(status)) => {
            let detail = tail
                .lock()
                .unwrap()
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");
            ItemResult::Failed(if detail.is_empty() {
                format!("ffmpeg 异常退出（{:?}）", status.code())
            } else {
                format!("ffmpeg 异常退出（{:?}）\n{detail}", status.code())
            })
        }
        Some(Err(error)) => ItemResult::Failed(format!("等待 ffmpeg 结束失败：{error}")),
        None => ItemResult::Failed("ffmpeg 进程已经结束".to_string()),
    }
}

/// 读 stderr：既留最后几行（失败原因），也顺手抓 `Duration:` 当进度分母。
fn read_stderr(
    stderr: impl Read,
    tail: Arc<Mutex<VecDeque<String>>>,
    duration: Arc<Mutex<Option<f64>>>,
) {
    for line in BufReader::new(stderr).lines().map_while(|line| line.ok()) {
        if let Some(seconds) = parse_duration(&line) {
            *duration.lock().unwrap() = Some(seconds);
        }
        let mut tail = tail.lock().unwrap();
        if tail.len() >= TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(line);
    }
}

/// 从 `  Duration: 00:01:30.00, start: ...` 里取总时长。`N/A` 返回 `None`。
fn parse_duration(line: &str) -> Option<f64> {
    let rest = line.trim_start().strip_prefix("Duration:")?;
    let stamp = rest.split(',').next()?.trim();
    let mut parts = stamp.split(':');
    let hours: f64 = parts.next()?.trim().parse().ok()?;
    let minutes: f64 = parts.next()?.trim().parse().ok()?;
    let seconds: f64 = parts.next()?.trim().parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(hours * 3600.0 + minutes * 60.0 + seconds)
}

/// 从 `-progress pipe:1` 的输出里读进度。
fn read_progress(
    stdout: impl Read,
    duration: &Arc<Mutex<Option<f64>>>,
    index: usize,
    tx: &UnboundedSender<ConvertEvent>,
) {
    for line in BufReader::new(stdout).lines().map_while(|line| line.ok()) {
        let Some(value) = line.strip_prefix("out_time_us=") else {
            continue;
        };
        let Ok(micros) = value.trim().parse::<f64>() else {
            continue;
        };
        let total = *duration.lock().unwrap();
        let ratio = total
            .filter(|seconds| *seconds > 0.0)
            .map(|seconds| (micros / (seconds * 1_000_000.0)).clamp(0.0, 1.0) as f32);
        let _ = tx.unbounded_send(ConvertEvent::Progress { index, ratio });
    }
}

/// 拼 ffmpeg 参数。
fn build_command(executable: &Path, request: &ConvertRequest) -> Command {
    let target = request.target;
    let mut command = Command::new(executable);
    command
        .arg("-hide_banner")
        .arg("-nostdin")
        .arg("-y")
        .arg("-nostats")
        // info 级别才打 `Duration:` 行，进度条的分母就从那里来；`-nostats` 已经
        // 关掉了每帧一次的统计，stderr 只多出开头那几行。
        .arg("-loglevel")
        .arg("info")
        .arg("-progress")
        .arg("pipe:1")
        .arg("-i")
        .arg(&request.input)
        .arg("-map_metadata")
        .arg("0");

    if target.keeps_video() {
        // 大写 V 排除封面图（attached_pic），否则第一路「视频」可能是一张 jpeg。
        command.arg("-map").arg("0:V:0?");
        // 裸流容器（h264 / m2v / y4m …）压根装不下音频轨，不能硬 map。
        if !target.audio_args().is_empty() {
            command.arg("-map").arg("0:a?");
        }
        if target.copy_subtitles() {
            // 必须先 map 再谈编码：只写 `-c:s copy` 不 map，字幕轨根本进不了输出。
            command.arg("-map").arg("0:s?").args(["-c:s", "copy"]);
        } else {
            command.arg("-sn");
        }
    } else {
        // 纯音频容器装不下一路以上的音频轨，只取第一条。
        command.arg("-map").arg("0:a:0?").arg("-vn").arg("-sn");
    }

    command
        .args(target.video_args())
        .args(target.audio_args())
        // 显式指定封装器：`mts` / `tp` / `alac` 这些扩展名 ffmpeg 猜不准，
        // 猜错时的报错信息是「无法找到合适的输出格式」，用户根本看不出问题在哪。
        .arg("-f")
        .arg(target.muxer())
        .arg(&request.output)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ngy-convert-task-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn target_mp4() -> TargetFormat {
        TargetFormat::by_extension("mp4").unwrap()
    }

    #[test]
    fn 换后缀且不覆盖原文件() {
        let dir = temp_dir("naming");
        let input = dir.join("课程 01.mkv");
        std::fs::write(&input, b"x").unwrap();

        let requests = plan(std::slice::from_ref(&input), target_mp4());
        assert_eq!(requests[0].output, dir.join("课程 01.mp4"));

        // 已经是 mp4：目标就是源文件自己，必须让开。
        let same = dir.join("课程 01.mp4");
        std::fs::write(&same, b"x").unwrap();
        let requests = plan(std::slice::from_ref(&same), target_mp4());
        assert_eq!(requests[0].output, dir.join("课程 01 - 转换.mp4"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn 同一批次内不撞名() {
        let dir = temp_dir("batch");
        let mkv = dir.join("a.mkv");
        let mp4 = dir.join("a.mp4");
        std::fs::write(&mkv, b"x").unwrap();
        std::fs::write(&mp4, b"x").unwrap();

        let requests = plan(&[mkv, mp4], target_mp4());
        assert_eq!(requests[0].output, dir.join("a - 转换.mp4"));
        assert_eq!(requests[1].output, dir.join("a - 转换 (2).mp4"));
        assert_ne!(requests[0].output, requests[1].output);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn 时长行能解析也认得_n_a() {
        assert_eq!(
            parse_duration("  Duration: 00:01:30.50, start: 0.000000, bitrate: 12 kb/s"),
            Some(90.5)
        );
        assert_eq!(parse_duration("  Duration: N/A, bitrate: N/A"), None);
        assert_eq!(parse_duration("  Stream #0:0"), None);
    }

    fn args_of(input: &str, output: &str, target: &str) -> Vec<String> {
        let request = ConvertRequest {
            input: std::path::PathBuf::from(input),
            output: std::path::PathBuf::from(output),
            target: TargetFormat::by_extension(target).unwrap(),
        };
        build_command(Path::new("ffmpeg"), &request)
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn 纯音频目标不带视频轨() {
        let args = args_of("in.mkv", "out.mp3", "mp3");
        assert!(args.iter().any(|arg| arg == "-vn"), "{args:?}");
        assert!(args.iter().any(|arg| arg == "0:a:0?"), "{args:?}");
        assert!(!args.iter().any(|arg| arg == "0:V:0?"), "{args:?}");
        // 封装器必须显式给：ffmpeg 按扩展名猜的时候，`mts`/`tp` 这类会猜空。
        assert!(
            args.windows(2).any(|pair| pair == ["-f", "mp3"]),
            "{args:?}"
        );
    }

    #[test]
    fn 视频目标映射字幕与音轨() {
        let args = args_of("in.mkv", "out.mkv", "mkv");
        assert!(args.iter().any(|arg| arg == "0:V:0?"), "{args:?}");
        assert!(args.iter().any(|arg| arg == "0:a?"), "{args:?}");
        assert!(args.iter().any(|arg| arg == "0:s?"), "{args:?}");
        assert!(
            args.windows(2).any(|pair| pair == ["-c:s", "copy"]),
            "{args:?}"
        );
        assert!(
            args.windows(2).any(|pair| pair == ["-f", "matroska"]),
            "{args:?}"
        );

        let args = args_of("in.mkv", "out.mp4", "mp4");
        assert!(args.iter().any(|arg| arg == "-sn"), "{args:?}");
        assert!(!args.iter().any(|arg| arg == "0:s"), "{args:?}");
    }

    #[test]
    fn 裸流容器不映射音轨() {
        // h264 / m2v / y4m 这些容器装不下音频，多 map 一路会被封装器拒绝。
        for extension in ["h264", "m2v", "y4m", "roq"] {
            let args = args_of("in.mkv", &format!("out.{extension}"), extension);
            assert!(
                args.iter().any(|arg| arg == "-an" || arg == "-sn"),
                "{args:?}"
            );
            assert!(
                !args.iter().any(|arg| arg == "0:a?"),
                "{extension}: {args:?}"
            );
        }
    }
}
