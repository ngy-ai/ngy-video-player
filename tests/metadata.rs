//! 章节 / 元数据编辑的端到端验证。
//!
//! 需要本机有 ffmpeg，默认忽略。这条测试只验证「参数能被 ffmpeg 接受、输出
//! 存在」；标签与章节的文本格式在 `src/metadata.rs` 的单测里逐字核对。

use futures::StreamExt as _;
use ngy_video_player::metadata::{
    ChapterEdit, MetadataEvent, MetadataJob, MetadataOutcome, MetadataRequest, Tags,
};
use std::path::{Path, PathBuf};
use std::process::Command;

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

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 重写章节与元数据() {
    let ffmpeg = ngy_video_player::platform::ffmpeg::executable().expect("本机没有 ffmpeg");
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
    };

    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let _job = MetadataJob::start(request, tx).expect("启动编辑失败");
    let outcome = futures::executor::block_on(async {
        while let Some(event) = rx.next().await {
            if let MetadataEvent::Done(outcome) = event {
                return outcome;
            }
        }
        panic!("任务没有回报结果");
    });

    match outcome {
        MetadataOutcome::Finished(path) => assert_eq!(path, output),
        other => panic!("编辑失败: {other:?}"),
    }
    assert!(output.is_file(), "输出文件不存在");
    assert!(std::fs::metadata(&output).unwrap().len() > 0);

    std::fs::remove_dir_all(&dir).unwrap();
}
