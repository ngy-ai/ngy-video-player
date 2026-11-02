//! 章节与容器元数据编辑：把标签和章节写进一个新文件。
//!
//! 画面、音轨、字幕、附件一律流复制，只有容器头里的 metadata 与 chapters
//! 换掉，所以一次编辑几秒结束。输出仍然是 mkv：章节文本文件与附件在 mp4 里
//! 的写法各不相同，换成 mkv 只需处理一套。
//!
//! 章节通过 ffmpeg 的 ffmetadata 文本文件写入（`-i chapters.txt -map_chapters 1`）。
//! 这是 ffmpeg 唯一稳定的批量章节入口，逐条 `-metadata` 没有对应的语法。

use anyhow::{Context as _, Result};
use futures::channel::mpsc::UnboundedSender;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 编辑输出的容器。
pub const OUTPUT_EXTENSION: &str = "mkv";

/// 输出文件名里追加的后缀。
const OUTPUT_SUFFIX: &str = "元数据";

/// 章节起点之间的最小间隔。比这更近的两条在播放器里永远选不中后一条。
const MIN_CHAPTER_GAP: f64 = 0.1;

/// 容器里的全局标签。空字符串表示「写一个空值」，也就是删除这一项。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tags {
    pub title: String,
    pub artist: String,
    pub comment: String,
}

/// 一条章节。结束时间由下一条的起点推出，不单独存。
#[derive(Clone, Debug, PartialEq)]
pub struct ChapterEdit {
    pub title: String,
    pub start: f64,
}

/// 一次章节/元数据编辑请求。
#[derive(Clone, Debug)]
pub struct MetadataRequest {
    pub input: PathBuf,
    pub output: PathBuf,
    /// 原文件时长（秒）。章节结束时间与进度分母都用它。
    pub duration: f64,
    pub tags: Tags,
    pub chapters: Vec<ChapterEdit>,
    /// 原文件内置字幕的编码名，用来决定是复制还是转成 mkv 支持的编码。
    pub subtitle_codecs: Vec<String>,
}

/// 后台任务回报给界面的事件。
#[derive(Clone, Debug)]
pub enum MetadataEvent {
    /// 0.0–1.0。
    Progress(f32),
    Done(MetadataOutcome),
}

#[derive(Clone, Debug)]
pub enum MetadataOutcome {
    Finished(PathBuf),
    Cancelled,
    Failed(String),
}

/// 正在跑的 ffmpeg 子进程。界面只持有句柄：进度走事件通道，取消直接杀进程。
pub struct MetadataJob {
    child: Arc<Mutex<Option<Child>>>,
    cancelled: Arc<AtomicBool>,
}

impl MetadataJob {
    /// 启动后台编辑。命令拼装、章节文件写入或进程启动失败会立即返回错误。
    pub fn start(request: MetadataRequest, tx: UnboundedSender<MetadataEvent>) -> Result<Self> {
        let executable = crate::platform::ffmpeg::executable()?;
        let chapters_file = if request.chapters.is_empty() {
            None
        } else {
            Some(write_chapters_file(&request)?)
        };
        let mut child = match build_command(&executable, &request, chapters_file.as_deref()).spawn()
        {
            Ok(child) => child,
            Err(error) => {
                // 进程都没起来，章节文件别留在临时目录里。
                if let Some(path) = &chapters_file {
                    let _ = std::fs::remove_file(path);
                }
                return Err(anyhow::Error::from(error))
                    .with_context(|| format!("启动 ffmpeg 失败：{}", executable.display()));
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let child = Arc::new(Mutex::new(Some(child)));
        let cancelled = Arc::new(AtomicBool::new(false));

        // stderr 必须持续读走：管道塞满会让 ffmpeg 卡在写日志上。
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
            // 章节文件只是这次任务的中间产物，成功失败都要清掉。
            if let Some(path) = &chapters_file {
                let _ = std::fs::remove_file(path);
            }

            let outcome = if cancelled {
                let _ = std::fs::remove_file(&output);
                MetadataOutcome::Cancelled
            } else {
                match status {
                    Some(Ok(status)) if status.success() => MetadataOutcome::Finished(output),
                    Some(Ok(status)) => {
                        let detail = tail
                            .lock()
                            .unwrap()
                            .iter()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("\n");
                        MetadataOutcome::Failed(if detail.is_empty() {
                            format!("ffmpeg 异常退出（{:?}）", status.code())
                        } else {
                            format!("ffmpeg 异常退出（{:?}）\n{detail}", status.code())
                        })
                    }
                    Some(Err(error)) => {
                        MetadataOutcome::Failed(format!("等待 ffmpeg 结束失败：{error}"))
                    }
                    None => MetadataOutcome::Failed("ffmpeg 进程已经结束".to_string()),
                }
            };
            let _ = tx.unbounded_send(MetadataEvent::Done(outcome));
        });

        Ok(Self { child, cancelled })
    }

    /// 取消编辑。子进程会在几百毫秒内被回收，任务线程负责删掉半成品与章节文件。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
    }
}

impl Drop for MetadataJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// 编辑结果的落点：与原文件同目录，名字加「 - 元数据」，已存在就往后编号。
pub fn output_path(input: &Path) -> PathBuf {
    let dir = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "输出".to_string());
    let base = format!("{stem} - {OUTPUT_SUFFIX}");

    let mut candidate = dir.join(format!("{base}.{OUTPUT_EXTENSION}"));
    let mut index = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{base} ({index}).{OUTPUT_EXTENSION}"));
        index += 1;
        if index > 999 {
            break;
        }
    }
    candidate
}

/// 排序、去重、补默认标题。空标题与越界章节在这里就被挡掉，不留给 ffmpeg 报错。
pub fn normalize_chapters(mut chapters: Vec<ChapterEdit>, duration: f64) -> Vec<ChapterEdit> {
    chapters.retain(|chapter| chapter.start >= 0.0 && chapter.start < duration.max(0.0));
    chapters.sort_by(|a, b| a.start.total_cmp(&b.start));
    chapters.dedup_by(|a, b| (a.start - b.start).abs() < MIN_CHAPTER_GAP);
    for (index, chapter) in chapters.iter_mut().enumerate() {
        let title = chapter.title.trim();
        if title.is_empty() {
            chapter.title = format!("章节 {}", index + 1);
        } else {
            chapter.title = title.to_string();
        }
    }
    chapters
}

/// ffmetadata 文本。TIMEBASE 用毫秒，时间点全部取整，避免小数点在文本格式里
/// 被不同版本按不同精度解析。
fn chapters_text(chapters: &[ChapterEdit], duration: f64) -> String {
    let mut text = String::from(";FFMETADATA1\n");
    for (index, chapter) in chapters.iter().enumerate() {
        let start = (chapter.start * 1000.0).round().max(0.0) as i64;
        let next = chapters
            .get(index + 1)
            .map(|next| next.start)
            .unwrap_or(duration);
        let end = (next * 1000.0).round() as i64;
        let end = end.max(start + 1);
        text.push_str("[CHAPTER]\nTIMEBASE=1/1000\n");
        text.push_str(&format!("START={start}\nEND={end}\n"));
        text.push_str(&format!("title={}\n", escape_ffmetadata(&chapter.title)));
    }
    text
}

/// ffmetadata 的转义：`=`、`;`、`#`、反斜杠和换行都是语法字符。
fn escape_ffmetadata(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' | '=' | ';' | '#' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            '\n' => escaped.push_str("\\\n"),
            '\r' => {}
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// 写章节文件。放系统临时目录：输出目录可能是只读的网络盘/相册目录。
fn write_chapters_file(request: &MetadataRequest) -> Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or(0);
    let path =
        std::env::temp_dir().join(format!("ngy-chapters-{}-{nanos}.txt", std::process::id()));
    std::fs::write(&path, chapters_text(&request.chapters, request.duration))
        .with_context(|| format!("写章节文件失败：{}", path.display()))?;
    Ok(path)
}

/// 从 `-progress pipe:1` 的输出里读进度。
fn read_progress(stdout: impl Read, total: f64, tx: &UnboundedSender<MetadataEvent>) {
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
        let _ = tx.unbounded_send(MetadataEvent::Progress(progress));
    }
}

/// 拼 ffmpeg 参数。
fn build_command(
    executable: &Path,
    request: &MetadataRequest,
    chapters_file: Option<&Path>,
) -> Command {
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
        .arg(&request.input);
    if let Some(path) = chapters_file {
        command.arg("-i").arg(path);
    }

    command
        // 只保留能装进 mkv 的几类流：数据流（如时间码轨）复制进 mkv 会失败。
        .arg("-map")
        .arg("0:V:0?")
        .arg("-map")
        .arg("0:a?")
        .arg("-map")
        .arg("0:s?")
        .arg("-map")
        .arg("0:t?")
        .arg("-c")
        .arg("copy")
        .arg("-map_metadata")
        .arg("0")
        .arg("-map_chapters")
        .arg(if chapters_file.is_some() { "1" } else { "-1" });

    // 文本字幕（mp4 的 mov_text）复制进 mkv 会直接失败，按原编码换成 mkv 认得的。
    if !request.subtitle_codecs.is_empty() {
        command
            .arg("-c:s")
            .arg(crate::subtitle_codec::kept(&request.subtitle_codecs));
    }

    for (key, value) in [
        ("title", &request.tags.title),
        ("artist", &request.tags.artist),
        ("comment", &request.tags.comment),
    ] {
        command
            .arg("-metadata")
            .arg(format!("{key}={}", value.trim()));
    }

    command
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

    fn request(chapters: Vec<ChapterEdit>) -> MetadataRequest {
        MetadataRequest {
            input: PathBuf::from("movie.mp4"),
            output: PathBuf::from("movie - 元数据.mkv"),
            duration: 600.0,
            tags: Tags {
                title: "新标题".into(),
                artist: "作者".into(),
                comment: String::new(),
            },
            chapters,
            subtitle_codecs: vec!["mov_text".into()],
        }
    }

    fn args(request: &MetadataRequest, chapters: Option<&Path>) -> Vec<String> {
        build_command(Path::new("ffmpeg"), request, chapters)
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
    fn 章节按时间排序去重并补默认标题() {
        let chapters = normalize_chapters(
            vec![
                ChapterEdit {
                    title: "  ".into(),
                    start: 30.0,
                },
                ChapterEdit {
                    title: "开场".into(),
                    start: 0.0,
                },
                ChapterEdit {
                    title: "重复".into(),
                    start: 30.05,
                },
                ChapterEdit {
                    title: "越界".into(),
                    start: 600.0,
                },
            ],
            600.0,
        );
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0].title, "开场");
        assert_eq!(chapters[1].title, "章节 2");
        assert_eq!(chapters[1].start, 30.0);
    }

    #[test]
    fn 章节文本用毫秒并转义语法字符() {
        let text = chapters_text(
            &[
                ChapterEdit {
                    title: "a=b;c#d\\e".into(),
                    start: 1.5,
                },
                ChapterEdit {
                    title: "结尾".into(),
                    start: 10.0,
                },
            ],
            12.0,
        );
        assert!(text.starts_with(";FFMETADATA1\n"));
        assert!(text.contains("START=1500\nEND=10000\n"));
        assert!(text.contains("title=a\\=b\\;c\\#d\\\\e\n"));
        // 最后一条的 END 取总时长，不能是 0。
        assert!(text.contains("START=10000\nEND=12000\n"));
    }

    #[test]
    fn 命令流复制并写入三项标签() {
        let args = args(&request(Vec::new()), None);
        assert!(has(&args, &["-c", "copy"]));
        assert!(has(&args, &["-metadata", "title=新标题"]));
        assert!(has(&args, &["-metadata", "artist=作者"]));
        assert!(has(&args, &["-metadata", "comment="]));
        assert!(has(&args, &["-c:s", "srt"]), "mov_text 要转成 srt");
        assert!(!has(&args, &["-i", "chapters.txt"]));
    }

    #[test]
    fn 没有章节时移除原章节() {
        let args = args(&request(Vec::new()), None);
        assert!(has(&args, &["-map_chapters", "-1"]));
    }

    #[test]
    fn 有章节时从第二个输入取章节() {
        let chapters = PathBuf::from("chapters.txt");
        let args = args(
            &request(vec![ChapterEdit {
                title: "一".into(),
                start: 1.0,
            }]),
            Some(&chapters),
        );
        assert!(has(&args, &["-i", "chapters.txt"]));
        assert!(has(&args, &["-map_chapters", "1"]));
        // 章节输入不能被当成媒体流映射进输出。
        assert!(!has(&args, &["-map", "1"]));
    }

    #[test]
    fn 输出文件不覆盖已有文件() {
        let dir = std::env::temp_dir().join(format!("ngy-metadata-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("课程 01.mp4");
        std::fs::write(&input, b"x").unwrap();

        let first = output_path(&input);
        assert_eq!(first, dir.join("课程 01 - 元数据.mkv"));
        std::fs::write(&first, b"x").unwrap();

        let second = output_path(&input);
        assert_eq!(second, dir.join("课程 01 - 元数据 (2).mkv"));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
