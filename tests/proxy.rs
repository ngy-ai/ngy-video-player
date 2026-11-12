//! 代理生成的端到端验证：需要本机 ffmpeg，默认忽略。

use futures::StreamExt as _;
use ngy_video_player::proxy::{ProxyEvent, ProxyJob, ProxyOutcome, ProxyRequest};
use std::process::Command;

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 生成一份代理文件() {
    let ffmpeg = ngy_video_player::platform::ffmpeg::executable().expect("本机没有 ffmpeg");
    let dir = std::env::temp_dir().join(format!("ngy-proxy-e2e-{}", std::process::id()));
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
            "testsrc2=size=640x360:rate=25:duration=3",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=3",
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

    let output = dir.join("in.proxy.mp4");
    let request = ProxyRequest {
        input,
        output: output.clone(),
        duration: 3.0,
        quality: ngy_video_player::proxy::ProxyQuality::P720,
    };
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let _job = ProxyJob::start(request, tx).expect("启动代理失败");
    let outcome = futures::executor::block_on(async {
        while let Some(event) = rx.next().await {
            if let ProxyEvent::Done(outcome) = event {
                return outcome;
            }
        }
        panic!("任务没有回报结果");
    });

    match outcome {
        ProxyOutcome::Finished(path) => assert_eq!(path, output),
        other => panic!("代理生成失败: {other:?}"),
    }
    assert!(output.is_file(), "代理文件不存在");
    assert!(std::fs::metadata(&output).unwrap().len() > 0);

    std::fs::remove_dir_all(&dir).unwrap();
}
