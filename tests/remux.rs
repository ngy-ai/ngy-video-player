//! 轨道编辑的端到端验证：换音轨、删音轨、内嵌字幕。
//!
//! 需要本机有 ffmpeg，默认忽略：CI 里没有它。纯逻辑（参数拼装、输出命名）在
//! `src/remux.rs` 的单测里，这里只验证「真能把轨道换进输出文件」。

use futures::StreamExt as _;
use ngy_video_player::remux::{
    RemuxEvent, RemuxJob, RemuxMode, RemuxOutcome, RemuxRequest, TrackAction, TrackTag,
};
use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 换音轨删音轨与内嵌字幕() {
    let ffmpeg = ngy_video_player::platform::ffmpeg::executable().expect("本机没有 ffmpeg");
    let dir = std::env::temp_dir().join(format!("ngy-remux-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let input = dir.join("in.mp4");
    generate(&ffmpeg, &input);
    let music = dir.join("music.m4a");
    Command::new(&ffmpeg)
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=880:duration=4",
            "-c:a",
            "aac",
        ])
        .arg(&music)
        .status()
        .expect("生成测试音频失败");
    let subtitle = dir.join("sub.srt");
    std::fs::write(
        &subtitle,
        "1\n00:00:00,000 --> 00:00:02,000\n你好\n\n2\n00:00:02,000 --> 00:00:04,000\n世界\n",
    )
    .unwrap();

    // 换音轨 + 内嵌字幕：输出应当只有一路视频、一路音频、一路字幕。
    let replaced = run(&RemuxRequest {
        mode: RemuxMode::Edit,
        input: input.clone(),
        output: dir.join("both.mkv"),
        audio: TrackAction::Replace(music),
        subtitle: TrackAction::Replace(subtitle.clone()),
        audio_tags: Vec::new(),
        subtitle_tags: Vec::new(),
        duration: 4.0,
        subtitle_codecs: Vec::new(),
    });
    let streams = describe(&ffmpeg, &replaced);
    assert_eq!(count(&streams, "Audio:"), 1, "{streams}");
    assert_eq!(count(&streams, "Subtitle:"), 1, "{streams}");
    assert_eq!(count(&streams, "Video:"), 1, "{streams}");

    // 删音轨：画面还在，音频没了。
    let muted = run(&RemuxRequest {
        mode: RemuxMode::Edit,
        input: input.clone(),
        output: dir.join("muted.mkv"),
        audio: TrackAction::Remove,
        subtitle: TrackAction::Keep,
        audio_tags: Vec::new(),
        subtitle_tags: Vec::new(),
        duration: 4.0,
        subtitle_codecs: Vec::new(),
    });
    let streams = describe(&ffmpeg, &muted);
    assert_eq!(count(&streams, "Audio:"), 0, "{streams}");
    assert_eq!(count(&streams, "Video:"), 1, "{streams}");

    // 只内嵌字幕：原有的音轨按原样保留。
    let subtitled = run(&RemuxRequest {
        mode: RemuxMode::Edit,
        input,
        output: dir.join("subbed.mkv"),
        audio: TrackAction::Keep,
        subtitle: TrackAction::Replace(subtitle),
        audio_tags: Vec::new(),
        subtitle_tags: Vec::new(),
        duration: 4.0,
        subtitle_codecs: Vec::new(),
    });
    let streams = describe(&ffmpeg, &subtitled);
    assert_eq!(count(&streams, "Audio:"), 1, "{streams}");
    assert_eq!(count(&streams, "Subtitle:"), 1, "{streams}");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 改写语言标签与默认轨() {
    let ffmpeg = ngy_video_player::platform::ffmpeg::executable().expect("本机没有 ffmpeg");
    let dir = std::env::temp_dir().join(format!("ngy-remux-tags-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let input = dir.join("in.mp4");
    generate(&ffmpeg, &input);
    let tagged = run(&RemuxRequest {
        mode: RemuxMode::Edit,
        input,
        output: dir.join("tagged.mkv"),
        audio_tags: vec![TrackTag {
            language: "jpn".into(),
            default: true,
        }],
        subtitle_tags: Vec::new(),
        audio: TrackAction::Keep,
        subtitle: TrackAction::Keep,
        duration: 4.0,
        subtitle_codecs: Vec::new(),
    });

    let streams = describe(&ffmpeg, &tagged);
    assert!(streams.contains("jpn"), "语言没写进去：{streams}");
    assert!(streams.contains("(default)"), "默认轨没标上：{streams}");

    std::fs::remove_dir_all(&dir).unwrap();
}

/// 生成一段带音轨的测试视频。
fn generate(ffmpeg: &Path, input: &Path) {
    let status = Command::new(ffmpeg)
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x120:rate=25:duration=4",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=4",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
        ])
        .arg(input)
        .status()
        .expect("生成测试视频失败");
    assert!(status.success());
}

/// 跑一次编辑，等它回报结果；成功时返回输出路径。
fn run(request: &RemuxRequest) -> PathBuf {
    let output = request.output.clone();
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let _job = RemuxJob::start(request.clone(), tx).expect("启动编辑失败");

    let outcome = futures::executor::block_on(async {
        while let Some(event) = rx.next().await {
            if let RemuxEvent::Done(outcome) = event {
                return outcome;
            }
        }
        panic!("任务没有回报结果");
    });

    match outcome {
        RemuxOutcome::Finished(path) => assert_eq!(path, output),
        other => panic!("编辑失败: {other:?}"),
    }
    assert!(output.is_file(), "输出文件不存在");
    assert!(std::fs::metadata(&output).unwrap().len() > 0);
    output
}

/// 让 ffmpeg 把文件的流信息打到 stderr 上；`-i` 没有输出文件，退出码不用管。
fn describe(ffmpeg: &Path, path: &Path) -> String {
    let result = Command::new(ffmpeg)
        .arg("-hide_banner")
        .arg("-i")
        .arg(path)
        .output()
        .expect("读取流信息失败");
    String::from_utf8_lossy(&result.stderr).into_owned()
}

fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}
