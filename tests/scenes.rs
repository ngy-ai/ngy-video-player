//! 镜头分割的端到端验证：需要本机 ffmpeg，默认忽略。

use futures::StreamExt as _;
use ngy_video_player::scenes::{SceneEvent, SceneJob, SceneOutcome, SceneRequest};
use std::process::Command;

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 三段颜色拼起来的片子有两个切点() {
    let ffmpeg = ngy_video_player::platform::ffmpeg::executable().expect("本机没有 ffmpeg");
    let dir = std::env::temp_dir().join(format!("ngy-scenes-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("in.mp4");

    // 红 2 秒 + 蓝 2 秒 + 绿 2 秒：切点应该在 2 秒与 4 秒附近。
    let generated = Command::new(&ffmpeg)
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=160x120:d=2:r=25",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=160x120:d=2:r=25",
            "-f",
            "lavfi",
            "-i",
            "color=c=green:s=160x120:d=2:r=25",
            "-filter_complex",
            "[0:v][1:v][2:v]concat=n=3:v=1:a=0",
        ])
        .arg(&input)
        .status()
        .expect("生成测试视频失败");
    assert!(generated.success());

    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let _job = SceneJob::start(
        SceneRequest {
            input,
            duration: 6.0,
            threshold: 0.3,
        },
        tx,
    )
    .expect("启动检测失败");

    let mut cuts = Vec::new();
    let mut progress_seen = false;
    let outcome = futures::executor::block_on(async {
        while let Some(event) = rx.next().await {
            match event {
                SceneEvent::Progress(progress) => progress_seen |= progress > 0.0,
                SceneEvent::Found(time) => cuts.push(time),
                SceneEvent::Done(outcome) => return outcome,
            }
        }
        panic!("任务没有回报结果");
    });
    assert!(
        matches!(outcome, SceneOutcome::Finished),
        "检测失败: {outcome:?}"
    );
    assert!(progress_seen, "没有收到进度，进度条会一直是 0");
    assert_eq!(cuts.len(), 2, "切点数量不对：{cuts:?}");
    assert!((cuts[0] - 2.0).abs() < 0.2, "第一个切点不对：{cuts:?}");
    assert!((cuts[1] - 4.0).abs() < 0.2, "第二个切点不对：{cuts:?}");

    std::fs::remove_dir_all(&dir).unwrap();
}
