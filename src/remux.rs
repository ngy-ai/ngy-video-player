//! 轨道编辑：把内置音轨换成指定文件、整条删掉，或者把字幕内嵌进容器。
//!
//! 和裁切不同，这里**不重新编码画面**：视频与保留下来的轨道一律流复制，只有被
//! 替换的那一路才动——音频按原编码搬进 mkv，字幕转成容器认得的编码。所以一次
//! 编辑通常几秒结束；但源文件在机械盘/网络盘上时仍要复制整条视频流，任务照样
//! 放在后台线程，并用 ffmpeg 的 `-progress` 回报进度，取消时杀掉子进程删掉半成品。
//!
//! 输出统一为 mkv：它能原样装下几乎任何编码，换成 mp4 就要为 PGS/ASS/DTS 找替身，
//! 失败面大得多。

use anyhow::{Context as _, Result};
use futures::channel::mpsc::UnboundedSender;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 编辑输出的容器。
pub const OUTPUT_EXTENSION: &str = "mkv";

/// 输出文件名里追加的后缀，用来和原文件、裁切结果区分开。
const OUTPUT_SUFFIX: &str = "编辑";

/// 只导出音轨时的容器。mka 是「去掉画面的 mkv」，任何音轨编码都能原样装进去，
/// 换成 mp3/m4a 就得为 DTS、TrueHD、FLAC 找替代方案，反而会掉质量。
pub const AUDIO_EXTENSION: &str = "mka";

/// 只导出音轨时输出文件名里追加的后缀。
const AUDIO_SUFFIX: &str = "音轨";

/// 这次任务做什么。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RemuxMode {
    /// 编辑轨道：画面流复制，音轨/字幕按 `TrackAction` 处置。
    #[default]
    Edit,
    /// 只导出音轨：丢掉画面与字幕，音轨原样复制进 mka。
    ExtractAudio,
}

impl RemuxMode {
    pub fn output_extension(self) -> &'static str {
        match self {
            RemuxMode::Edit => OUTPUT_EXTENSION,
            RemuxMode::ExtractAudio => AUDIO_EXTENSION,
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            RemuxMode::Edit => OUTPUT_SUFFIX,
            RemuxMode::ExtractAudio => AUDIO_SUFFIX,
        }
    }
}

/// 某一路轨道在输出里的处置方式。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrackAction {
    /// 原样保留（流复制）。
    Keep,
    /// 换成这个文件里的第一路；输出里只留这一条。
    Replace(PathBuf),
    /// 这一类轨道全部不要。
    Remove,
}

impl TrackAction {
    pub fn is_keep(&self) -> bool {
        matches!(self, TrackAction::Keep)
    }
}

/// 一条内置轨道的语言与默认标记。
///
/// 只在「保留原轨」时有意义：换上的新轨只有一个，直接写 `:0` 就够了。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrackTag {
    /// ISO 639 语言代码（`chi`、`eng`…）。空字符串表示不写，沿用源里的值。
    pub language: String,
    /// 输出里把它标成默认轨。同一类轨道里应当只有一条。
    pub default: bool,
}

impl TrackTag {
    /// 语言代码收成小写并去掉空白：mkv 里存的就是 `chi` 这种写法。
    pub fn normalized(language: &str) -> String {
        language.trim().to_ascii_lowercase()
    }

    /// 2–3 个字母的代码才认。`chinese` 这种全称 ffmpeg 会原样写进容器，
    /// 播放器却按代码解释，结果就是「语言显示不出来」。
    pub fn valid_language(language: &str) -> bool {
        let language = Self::normalized(language);
        (2..=3).contains(&language.len()) && language.chars().all(|ch| ch.is_ascii_alphabetic())
    }
}

/// 一次轨道编辑请求。
#[derive(Clone, Debug)]
pub struct RemuxRequest {
    pub mode: RemuxMode,
    pub input: PathBuf,
    pub output: PathBuf,
    /// 保留下来的每条音轨的语言与默认标记，按输出顺序排。空表示什么都不改。
    pub audio_tags: Vec<TrackTag>,
    /// 保留下来的每条字幕轨的语言与默认标记。
    pub subtitle_tags: Vec<TrackTag>,
    /// 音轨：保留 / 换成这个文件 / 删掉。
    pub audio: TrackAction,
    /// 字幕轨：保留 / 换成这个文件 / 删掉。
    pub subtitle: TrackAction,
    /// 原文件时长（秒）。只用来把 `out_time_us` 换算成百分比。
    pub duration: f64,
    /// 原文件内置字幕轨的编码名，取自播放器快照。
    ///
    /// 「保留字幕」时要按它决定输出编码：点阵字幕（PGS/DVD）只能流复制，ASS 要
    /// 留住样式，其余（srt、mp4 的 mov_text……）转成 srt 才装得进 mkv。
    pub subtitle_codecs: Vec<String>,
}

/// 后台任务回报给界面的事件。
#[derive(Clone, Debug)]
pub enum RemuxEvent {
    /// 0.0–1.0。
    Progress(f32),
    Done(RemuxOutcome),
}

#[derive(Clone, Debug)]
pub enum RemuxOutcome {
    Finished(PathBuf),
    Cancelled,
    Failed(String),
}

/// 正在跑的 ffmpeg 子进程。界面只持有句柄：进度走事件通道，取消直接杀进程。
pub struct RemuxJob {
    child: Arc<Mutex<Option<Child>>>,
    cancelled: Arc<AtomicBool>,
}

impl RemuxJob {
    /// 启动后台编辑。命令拼装或进程启动失败会立即返回错误，界面不必等事件。
    pub fn start(request: RemuxRequest, tx: UnboundedSender<RemuxEvent>) -> Result<Self> {
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
        let total = request.duration;
        std::thread::spawn(move || {
            if let Some(stdout) = stdout {
                read_progress(stdout, total, &tx);
            }
            let status = wait_polling(&task_child);
            let cancelled = task_cancelled.load(Ordering::SeqCst);

            let outcome = if cancelled {
                // 半成品留着只会让用户以为编辑成功了。
                let _ = std::fs::remove_file(&output);
                RemuxOutcome::Cancelled
            } else {
                match status {
                    Some(Ok(status)) if status.success() => RemuxOutcome::Finished(output),
                    Some(Ok(status)) => {
                        let detail = tail
                            .lock()
                            .unwrap()
                            .iter()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("\n");
                        RemuxOutcome::Failed(if detail.is_empty() {
                            format!("ffmpeg 异常退出（{:?}）", status.code())
                        } else {
                            format!("ffmpeg 异常退出（{:?}）\n{detail}", status.code())
                        })
                    }
                    Some(Err(error)) => {
                        RemuxOutcome::Failed(format!("等待 ffmpeg 结束失败：{error}"))
                    }
                    None => RemuxOutcome::Failed("ffmpeg 进程已经结束".to_string()),
                }
            };
            let _ = tx.unbounded_send(RemuxEvent::Done(outcome));
        });

        Ok(Self { child, cancelled })
    }

    /// 取消编辑。子进程会在几百毫秒内被回收，任务线程负责删掉半成品。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
    }
}

impl Drop for RemuxJob {
    fn drop(&mut self) {
        // 关窗口退出时不能把 ffmpeg 留在后台继续写文件。
        self.cancel();
    }
}

/// 编辑结果的落点：与原文件同目录，名字加「 - 编辑」，已存在就往后编号。
///
/// 不覆盖已有文件：编辑要复制整条视频流，一次手滑把同名文件盖掉就找不回来了。
pub fn output_path(input: &Path, mode: RemuxMode) -> PathBuf {
    let dir = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "输出".to_string());
    let base = format!("{stem} - {}", mode.suffix());
    let extension = mode.output_extension();

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

/// 轮询等待子进程退出。
///
/// 不用 `wait()` 是因为它要独占 `&mut Child`，取消时就没法杀进程了；100ms 的
/// 轮询延迟对「点取消」这件事完全够用。
fn wait_polling(
    child: &Arc<Mutex<Option<Child>>>,
) -> Option<std::io::Result<std::process::ExitStatus>> {
    loop {
        {
            let mut slot = child.lock().unwrap();
            let process = slot.as_mut()?;
            match process.try_wait() {
                Ok(Some(status)) => {
                    slot.take();
                    return Some(Ok(status));
                }
                Ok(None) => {}
                Err(error) => {
                    slot.take();
                    return Some(Err(error));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// 从 `-progress pipe:1` 的输出里读进度。
fn read_progress(stdout: impl Read, total: f64, tx: &UnboundedSender<RemuxEvent>) {
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
        let _ = tx.unbounded_send(RemuxEvent::Progress(progress));
    }
}

/// 拼 ffmpeg 参数。
///
/// 输出的流顺序固定为「画面 → 音轨 → 字幕」，附加输入也按「音轨在前、字幕在后」
/// 的顺序加，这样 `-map` 里的输入序号是可以直接算出来的。
fn build_command(executable: &Path, request: &RemuxRequest) -> Command {
    let mut command = Command::new(executable);
    command
        .arg("-hide_banner")
        .arg("-nostdin")
        .arg("-y")
        .arg("-nostats")
        .arg("-loglevel")
        .arg("error")
        .arg("-progress")
        .arg("pipe:1");

    // 只导出音轨走另一条完全不同的路：不需要任何轨道映射，只留第一路音频原样复制。
    // `?` 让没有音轨的文件也走到「失败并给出原因」，而不是留下一句空的输出文件。
    if request.mode == RemuxMode::ExtractAudio {
        command
            .arg("-i")
            .arg(&request.input)
            .arg("-vn")
            .arg("-map")
            .arg("0:a:0?")
            .arg("-c:a")
            .arg("copy")
            .arg("-map_metadata")
            .arg("0")
            .arg("-map_chapters")
            .arg("-1")
            .arg("-avoid_negative_ts")
            .arg("make_zero")
            .arg(&request.output)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        return command;
    }

    command.arg("-i").arg(&request.input);

    let audio_input = match &request.audio {
        TrackAction::Replace(path) => {
            command.arg("-i").arg(path);
            Some(1)
        }
        _ => None,
    };
    let subtitle_input = match &request.subtitle {
        TrackAction::Replace(path) => {
            command.arg("-i").arg(path);
            Some(if audio_input.is_some() { 2 } else { 1 })
        }
        _ => None,
    };

    // 大写 V 排除封面图（attached_pic），否则第一路「视频」可能是一张 jpeg。
    command.arg("-map").arg("0:V:0?");
    match &request.audio {
        TrackAction::Keep => {
            command.arg("-map").arg("0:a?");
            command.arg("-c:a").arg("copy");
        }
        TrackAction::Replace(_) => {
            command
                .arg("-map")
                .arg(format!("{}:a:0", audio_input.unwrap_or(1)));
            // 不重编码：mkv 装得下常见音频编码（AAC/MP3/FLAC/Opus/AC3……），
            // 转码只会平白掉质量，也拖慢整个操作。
            command.arg("-c:a").arg("copy");
        }
        TrackAction::Remove => {}
    }
    match &request.subtitle {
        TrackAction::Keep => {
            command.arg("-map").arg("0:s?");
            command
                .arg("-c:s")
                .arg(crate::subtitle_codec::kept(&request.subtitle_codecs));
        }
        TrackAction::Replace(path) => {
            command
                .arg("-map")
                .arg(format!("{}:s:0", subtitle_input.unwrap_or(1)));
            command
                .arg("-c:s")
                .arg(crate::subtitle_codec::from_path(path));
        }
        TrackAction::Remove => {}
    }

    command
        .arg("-map_metadata")
        .arg("0")
        .arg("-map_chapters")
        .arg("0")
        // 画面一律复制：编辑轨道不该动用户的画质。
        .arg("-c:v")
        .arg("copy")
        .arg("-avoid_negative_ts")
        .arg("make_zero");

    // 换进来的轨道挂上文件名当标题，播放器的轨道菜单里才分得清是谁。
    // 放在 `-map_metadata` 之后，覆盖掉主输入同序号轨道的元数据。
    for (specifier, action) in [("a", &request.audio), ("s", &request.subtitle)] {
        if let TrackAction::Replace(path) = action
            && let Some(title) = file_title(path)
        {
            command
                .arg(format!("-metadata:s:{specifier}:0"))
                .arg(format!("title={title}"));
        }
    }

    // 语言与默认轨：只对「保留原轨」生效，替换后的输出里只剩一条，改它没有意义。
    apply_tags(
        &mut command,
        "a",
        &request.audio_tags,
        request.audio.is_keep(),
    );
    apply_tags(
        &mut command,
        "s",
        &request.subtitle_tags,
        request.subtitle.is_keep(),
    );

    command
        .arg(&request.output)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// 逐轨写语言与默认标记。
///
/// 一旦标记了某条默认轨，其余轨道必须显式清掉 default：容器里允许同时存在多条
/// 「默认」，播放器按存储顺序挑，用户设的那条未必生效。没标任何默认轨时干脆不碰
/// 处置标记，原文件的默认轨设置原样保留。
fn apply_tags(command: &mut Command, specifier: &str, tags: &[TrackTag], kept: bool) {
    if !kept {
        return;
    }
    let any_default = tags.iter().any(|tag| tag.default);
    for (index, tag) in tags.iter().enumerate() {
        let language = TrackTag::normalized(&tag.language);
        if !language.is_empty() {
            command
                .arg(format!("-metadata:s:{specifier}:{index}"))
                .arg(format!("language={language}"));
        }
        if any_default {
            command
                .arg(format!("-disposition:{specifier}:{index}"))
                .arg(if tag.default { "default" } else { "0" });
        }
    }
}

/// 新轨道的标题：文件名去掉扩展名，空名字就不要标题。
fn file_title(path: &Path) -> Option<String> {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().trim().to_string())
        .filter(|stem| !stem.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(audio: TrackAction, subtitle: TrackAction) -> RemuxRequest {
        RemuxRequest {
            mode: RemuxMode::Edit,
            input: PathBuf::from("movie.mkv"),
            output: PathBuf::from("movie - 编辑.mkv"),
            audio_tags: Vec::new(),
            subtitle_tags: Vec::new(),
            audio,
            subtitle,
            duration: 600.0,
            subtitle_codecs: Vec::new(),
        }
    }

    fn extract() -> RemuxRequest {
        RemuxRequest {
            mode: RemuxMode::ExtractAudio,
            input: PathBuf::from("movie.mkv"),
            output: PathBuf::from("movie - 音轨.mka"),
            audio_tags: Vec::new(),
            subtitle_tags: Vec::new(),
            audio: TrackAction::Keep,
            subtitle: TrackAction::Remove,
            duration: 600.0,
            subtitle_codecs: Vec::new(),
        }
    }

    fn args(request: &RemuxRequest) -> Vec<String> {
        build_command(Path::new("ffmpeg"), request)
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    /// 参数里是否出现过这组连续的片段。
    fn has(args: &[String], needle: &[&str]) -> bool {
        args.windows(needle.len()).any(|slice| {
            slice
                .iter()
                .zip(needle)
                .all(|(arg, want)| arg.as_str() == *want)
        })
    }

    fn count(args: &[String], value: &str) -> usize {
        args.iter().filter(|arg| arg.as_str() == value).count()
    }

    #[test]
    fn 删除音轨后输出不再有音频() {
        let args = args(&request(TrackAction::Remove, TrackAction::Keep));
        assert!(has(&args, &["-map", "0:V:0?"]));
        assert!(!has(&args, &["-map", "0:a?"]));
        assert!(
            !args.iter().any(|arg| arg == "-c:a"),
            "没有音轨就不该给音频编码"
        );
        assert!(has(&args, &["-map", "0:s?"]));
        assert!(has(&args, &["-c:v", "copy"]));
    }

    #[test]
    fn 替换音轨时多一路输入且只留一条音轨() {
        let args = args(&request(
            TrackAction::Replace(PathBuf::from("music.flac")),
            TrackAction::Keep,
        ));
        assert_eq!(count(&args, "-i"), 2, "主输入 + 音频输入");
        assert!(has(&args, &["-map", "1:a:0"]));
        assert!(!has(&args, &["-map", "0:a?"]), "原音轨不该再进输出");
        assert!(has(&args, &["-c:a", "copy"]));
        assert!(has(&args, &["-metadata:s:a:0", "title=music"]));
    }

    #[test]
    fn 替换字幕按扩展名选编码() {
        for (name, codec) in [
            ("sub.srt", "srt"),
            ("sub.vtt", "srt"),
            ("sub.ass", "ass"),
            ("sub.ssa", "ass"),
            ("sub.sup", "copy"),
        ] {
            let args = args(&request(
                TrackAction::Keep,
                TrackAction::Replace(PathBuf::from(name)),
            ));
            assert!(has(&args, &["-map", "1:s:0"]), "{name}");
            assert!(!has(&args, &["-map", "0:s?"]), "{name}");
            assert!(has(&args, &["-c:s", codec]), "{name}");
        }
    }

    #[test]
    fn 同时替换音轨与字幕时输入序号递增() {
        let args = args(&request(
            TrackAction::Replace(PathBuf::from("music.m4a")),
            TrackAction::Replace(PathBuf::from("sub.srt")),
        ));
        assert_eq!(count(&args, "-i"), 3);
        assert!(has(&args, &["-map", "1:a:0"]));
        assert!(has(&args, &["-map", "2:s:0"]));
    }

    #[test]
    fn 移除字幕时不再映射字幕() {
        let args = args(&request(TrackAction::Keep, TrackAction::Remove));
        assert!(!has(&args, &["-map", "0:s?"]));
        assert!(!args.iter().any(|arg| arg == "-c:s"));
        assert!(has(&args, &["-map", "0:a?"]));
    }

    #[test]
    fn 输出文件不覆盖已有文件() {
        let dir = std::env::temp_dir().join(format!("ngy-remux-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("课程 01.mp4");
        std::fs::write(&input, b"x").unwrap();

        let first = output_path(&input, RemuxMode::Edit);
        assert_eq!(first, dir.join("课程 01 - 编辑.mkv"));
        std::fs::write(&first, b"x").unwrap();

        let second = output_path(&input, RemuxMode::Edit);
        assert_eq!(second, dir.join("课程 01 - 编辑 (2).mkv"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn 只导出音轨时没有画面只有一路音频() {
        let args = args(&extract());
        assert!(has(&args, &["-vn"]));
        assert!(has(&args, &["-map", "0:a:0?"]));
        assert!(has(&args, &["-c:a", "copy"]));
        assert!(!has(&args, &["-c:v", "copy"]), "不该再映射画面");
        assert!(!has(&args, &["-map", "0:s?"]), "字幕不跟着导出");
        assert!(has(&args, &["-map_chapters", "-1"]), "音轨文件不挂章节");
        assert!(args.iter().any(|arg| arg.ends_with(" - 音轨.mka")));
    }

    #[test]
    fn 保留轨道时逐条写语言与默认标记() {
        let mut request = request(TrackAction::Keep, TrackAction::Keep);
        request.audio_tags = vec![
            TrackTag {
                language: " Chi ".into(),
                default: false,
            },
            TrackTag {
                language: "eng".into(),
                default: true,
            },
        ];
        let args = args(&request);
        assert!(has(&args, &["-metadata:s:a:0", "language=chi"]));
        assert!(has(&args, &["-metadata:s:a:1", "language=eng"]));
        // 标了默认轨之后，其余轨道要显式清掉 default。
        assert!(has(&args, &["-disposition:a:0", "0"]));
        assert!(has(&args, &["-disposition:a:1", "default"]));
    }

    #[test]
    fn 没有默认轨时不碰处置标记() {
        let mut request = request(TrackAction::Keep, TrackAction::Keep);
        request.audio_tags = vec![TrackTag {
            language: "jpn".into(),
            default: false,
        }];
        let args = args(&request);
        assert!(has(&args, &["-metadata:s:a:0", "language=jpn"]));
        assert!(!args.iter().any(|arg| arg.starts_with("-disposition:")));
    }

    #[test]
    fn 替换轨道后不再写保留轨的标签() {
        let mut request = request(
            TrackAction::Replace(PathBuf::from("music.flac")),
            TrackAction::Keep,
        );
        request.audio_tags = vec![TrackTag {
            language: "eng".into(),
            default: true,
        }];
        let args = args(&request);
        // 替换那一路会写标题，但不能写出语言与默认标记。
        assert!(!has(&args, &["-metadata:s:a:0", "language=eng"]));
        assert!(!args.iter().any(|arg| arg.starts_with("-disposition:a:")));
    }

    #[test]
    fn 语言代码只认两三个字母() {
        assert!(TrackTag::valid_language("chi"));
        assert!(TrackTag::valid_language(" zh "));
        assert!(!TrackTag::valid_language("chinese"));
        assert!(!TrackTag::valid_language("1"));
        assert!(!TrackTag::valid_language(""));
    }

    #[test]
    fn 只导出音轨的落点是_mka() {
        let input = PathBuf::from("sample.mp4");
        let output = output_path(&input, RemuxMode::ExtractAudio);
        assert_eq!(output, PathBuf::from("sample - 音轨.mka"));
    }
}
