//! 章节 / 元数据编辑的端到端验证。
//!
//! 需要本机有 ffmpeg，默认忽略。这些测试验证「参数能被 ffmpeg 接受、输出的流与
//! 附件结构对得上」；标签与章节的文本格式在 `src/metadata/tests.rs` 里逐字核对。

use futures::StreamExt as _;
use ngy_video_player::metadata::{
    ChapterEdit, MetadataEvent, MetadataJob, MetadataOutcome, MetadataRequest, Tags,
};
use std::path::{Path, PathBuf};
use std::process::Command;

fn ffmpeg() -> PathBuf {
    ngy_video_player::platform::ffmpeg::executable().expect("本机没有 ffmpeg")
}

/// 跑一次编辑，等到任务结束。
fn run(request: MetadataRequest) -> MetadataOutcome {
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let _job = MetadataJob::start(request, tx).expect("启动编辑失败");
    futures::executor::block_on(async {
        while let Some(event) = rx.next().await {
            if let MetadataEvent::Done(outcome) = event {
                return outcome;
            }
        }
        panic!("任务没有回报结果");
    })
}

/// `ffmpeg -i` 的文本输出，用来看流与附件。
fn stream_info(ffmpeg: &Path, path: &Path) -> String {
    let info = Command::new(ffmpeg)
        .arg("-hide_banner")
        .arg("-i")
        .arg(path)
        .output()
        .expect("读取流信息失败");
    String::from_utf8_lossy(&info.stderr).into_owned()
}

fn sample(ffmpeg: &Path, dir: &Path) -> PathBuf {
    let input = dir.join("in.mp4");
    let generated = Command::new(ffmpeg)
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x120:rate=25:duration=6",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=6",
            "-c:v",
            "libx264",
            "-g",
            "25",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-metadata",
            "title=旧标题",
        ])
        .arg(&input)
        .status()
        .expect("生成测试视频失败");
    assert!(generated.success());
    input
}

fn make_cover(ffmpeg: &Path, dir: &Path) -> PathBuf {
    let cover = dir.join("cover.jpg");
    let generated = Command::new(ffmpeg)
        .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg("color=c=red:s=120x120:d=1")
        .args(["-frames:v", "1"])
        .arg(&cover)
        .status()
        .expect("生成封面失败");
    assert!(generated.success());
    cover
}

/// 生成一个自带封面的 mkv。mkv 的封面是图片附件，ffmpeg 读回来是 attached_pic
/// 视频流——这正是「只改标题也不能丢封面」要覆盖的输入。
fn sample_with_cover(ffmpeg: &Path, dir: &Path) -> PathBuf {
    let base = dir.join("base.mkv");
    let generated = Command::new(ffmpeg)
        .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg("testsrc2=size=160x120:rate=25:duration=4")
        .args([
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-metadata",
            "title=旧标题",
        ])
        .arg(&base)
        .status()
        .expect("生成测试视频失败");
    assert!(generated.success());

    let cover = make_cover(ffmpeg, dir);
    let input = dir.join("cover-in.mkv");
    let attached = Command::new(ffmpeg)
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(&base)
        .arg("-attach")
        .arg(&cover)
        .args([
            "-metadata:s:t:0",
            "mimetype=image/jpeg",
            "-metadata:s:t:0",
            "filename=cover.jpg",
            "-c",
            "copy",
        ])
        .arg(&input)
        .status()
        .expect("附加封面失败");
    assert!(attached.success());
    input
}

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 重写章节与元数据() {
    let ffmpeg = ffmpeg();
    let dir = std::env::temp_dir().join(format!("ngy-metadata-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = sample(&ffmpeg, &dir);
    let output = dir.join("out.mkv");

    let request = MetadataRequest {
        input,
        output: output.clone(),
        duration: 6.0,
        tags: Tags {
            title: "新标题".into(),
            artist: "作者".into(),
            comment: String::new(),
        },
        chapters: vec![
            ChapterEdit {
                title: "开场".into(),
                start: 0.0,
            },
            ChapterEdit {
                title: "第二段=带符号".into(),
                start: 2.5,
            },
        ],
        subtitle_codecs: Vec::new(),
        cover: None,
    };

    match run(request) {
        MetadataOutcome::Finished(path) => assert_eq!(path, output),
        other => panic!("编辑失败: {other:?}"),
    }
    assert!(output.is_file(), "输出文件不存在");
    assert!(std::fs::metadata(&output).unwrap().len() > 0);

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 封面图写成附件() {
    let ffmpeg = ffmpeg();
    let dir = std::env::temp_dir().join(format!("ngy-metadata-cover-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = sample(&ffmpeg, &dir);
    let cover = make_cover(&ffmpeg, &dir);

    let output = dir.join("cover-out.mkv");
    let request = MetadataRequest {
        input,
        output: output.clone(),
        duration: 6.0,
        tags: Tags::default(),
        chapters: Vec::new(),
        subtitle_codecs: Vec::new(),
        cover: Some(cover),
    };
    let outcome = run(request);
    assert!(
        matches!(outcome, MetadataOutcome::Finished(_)),
        "编辑失败: {outcome:?}"
    );

    // ffmpeg 把 `-attach` 的图片写成 attached_pic 轨，播放器按专辑封面显示。
    let text = stream_info(&ffmpeg, &output);
    assert!(text.contains("attached pic"), "封面没写进去：{text}");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 原文件已有附件时封面排在它们后面() {
    let ffmpeg = ffmpeg();
    let dir = std::env::temp_dir().join(format!("ngy-metadata-attach-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // 带一个附件的 mkv：mkv 里最常见的附件是 ASS 字体，这里用内容无所谓的假字体。
    let base = dir.join("base.mkv");
    let generated = Command::new(&ffmpeg)
        .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg("testsrc2=size=160x120:rate=25:duration=4")
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(&base)
        .status()
        .expect("生成测试视频失败");
    assert!(generated.success());

    let font = dir.join("font.ttf");
    std::fs::write(&font, b"fake font").unwrap();
    let input = dir.join("in.mkv");
    let attached = Command::new(&ffmpeg)
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(&base)
        .arg("-attach")
        .arg(&font)
        .args([
            "-metadata:s:t:0",
            "mimetype=application/x-truetype-font",
            "-c",
            "copy",
        ])
        .arg(&input)
        .status()
        .expect("附加字体失败");
    assert!(attached.success());

    let cover = make_cover(&ffmpeg, &dir);

    // 新封面的 mimetype 要写在原附件之后的序号上。序号算错时 mkv muxer 会因为
    // 新封面没有 mimetype 直接拒绝写头，任务会失败而不是悄悄丢封面。
    let output = dir.join("out.mkv");
    let request = MetadataRequest {
        input,
        output: output.clone(),
        duration: 4.0,
        tags: Tags::default(),
        chapters: Vec::new(),
        subtitle_codecs: Vec::new(),
        cover: Some(cover),
    };
    let outcome = run(request);
    assert!(
        matches!(outcome, MetadataOutcome::Finished(_)),
        "编辑失败: {outcome:?}"
    );

    let text = stream_info(&ffmpeg, &output);
    assert!(text.contains("attached pic"), "封面没写进去：{text}");
    // 原来的字体附件仍在，且没有被新封面的 mimetype 覆盖。
    assert!(
        text.lines()
            .any(|line| line.contains(": Attachment:") && line.contains("ttf")),
        "原附件丢了或被改坏：{text}"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 只改标题时原封面仍然在() {
    let ffmpeg = ffmpeg();
    let dir = std::env::temp_dir().join(format!("ngy-metadata-keep-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = sample_with_cover(&ffmpeg, &dir);

    // 不选新封面。`-map 0:V` 只带主视频，封面是独立的 attached_pic 流，
    // 不单独提取再按附件写回就会在「只改标题」时丢掉。
    let output = dir.join("keep-out.mkv");
    let request = MetadataRequest {
        input,
        output: output.clone(),
        duration: 4.0,
        tags: Tags {
            title: "新标题".into(),
            ..Tags::default()
        },
        chapters: Vec::new(),
        subtitle_codecs: Vec::new(),
        cover: None,
    };
    let outcome = run(request);
    assert!(
        matches!(outcome, MetadataOutcome::Finished(_)),
        "编辑失败: {outcome:?}"
    );

    let text = stream_info(&ffmpeg, &output);
    assert!(text.contains("attached pic"), "封面丢了：{text}");
    assert!(text.contains("新标题"), "标题没写进去：{text}");

    std::fs::remove_dir_all(&dir).unwrap();
}
