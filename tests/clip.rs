//! 裁切任务的端到端验证。
//!
//! 需要本机有 ffmpeg，默认忽略：CI 里没有它，而这条测试本身也只是在验证
//! 「参数拼对了、进度回来了」——纯逻辑部分在 `src/clip.rs` 的单测里。

use futures::StreamExt as _;
use ngy_video_player::clip::{ClipEvent, ClipJob, ClipOutcome, ClipRange, ClipRequest};
use std::process::Command;

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 裁切一段真实视频() {
    let ffmpeg = ngy_video_player::platform::ffmpeg::executable().expect("本机没有 ffmpeg");
    let dir = std::env::temp_dir().join(format!("ngy-clip-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let input = dir.join("in.mp4");
    let generated = Command::new(&ffmpeg)
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
        .arg(&input)
        .status()
        .expect("生成测试视频失败");
    assert!(generated.success());

    let output = dir.join("out.mkv");
    let request = ClipRequest {
        input,
        output: output.clone(),
        range: ClipRange {
            start: 1.0,
            end: 3.0,
        },
    };
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let _job = ClipJob::start(request, tx).expect("启动裁切失败");

    let (outcome, progress) = futures::executor::block_on(async {
        let mut progress = Vec::new();
        while let Some(event) = rx.next().await {
            match event {
                ClipEvent::Progress(value) => progress.push(value),
                ClipEvent::Done(outcome) => return (outcome, progress),
            }
        }
        panic!("任务没有回报结果");
    });

    match outcome {
        ClipOutcome::Finished(path) => assert_eq!(path, output),
        other => panic!("裁切失败: {other:?}"),
    }
    assert!(!progress.is_empty(), "应当收到进度事件");
    assert!(
        progress.last().is_some_and(|value| *value > 0.0),
        "进度应当往前走"
    );
    assert!(output.is_file());
    assert!(std::fs::metadata(&output).unwrap().len() > 0);

    std::fs::remove_dir_all(&dir).unwrap();
}
