//! 双机位拼屏的端到端验证：需要本机 ffmpeg，默认忽略。

use futures::StreamExt as _;
use ngy_video_player::multicam::{
    MulticamEvent, MulticamJob, MulticamOutcome, MulticamRequest, probe_duration,
};
use std::path::{Path, PathBuf};
use std::process::Command;

fn sample(ffmpeg: &Path, dir: &Path, name: &str, size: &str, rate: u32) -> PathBuf {
    let path = dir.join(name);
    let generated = Command::new(ffmpeg)
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc2=size={size}:rate={rate}:duration=3"),
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
        .arg(&path)
        .status()
        .expect("生成测试视频失败");
    assert!(generated.success());
    path
}

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 两个不同尺寸与帧率的机位可以拼屏() {
    let ffmpeg = ngy_video_player::platform::ffmpeg::executable().expect("本机没有 ffmpeg");
    let dir = std::env::temp_dir().join(format!("ngy-multicam-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // 故意用横屏 25fps 与竖屏 30fps：验证缩放补边与统一帧率。
    let left = sample(&ffmpeg, &dir, "left.mp4", "320x180", 25);
    let right = sample(&ffmpeg, &dir, "right.mp4", "180x320", 30);

    let left_duration = probe_duration(&left).expect("读不出左机位时长");
    let right_duration = probe_duration(&right).expect("读不出右机位时长");
    assert!((left_duration - 3.0).abs() < 0.3);
    assert!((right_duration - 3.0).abs() < 0.3);

    let output = dir.join("out.mp4");
    let request = MulticamRequest {
        left,
        right,
        right_skip: 0.5,
        duration: 2.0,
        output: output.clone(),
    };
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let _job = MulticamJob::start(request, tx).expect("启动拼屏失败");
    let outcome = futures::executor::block_on(async {
        while let Some(event) = rx.next().await {
            if let MulticamEvent::Done(outcome) = event {
                return outcome;
            }
        }
        panic!("任务没有回报结果");
    });

    match outcome {
        MulticamOutcome::Finished(path) => assert_eq!(path, output),
        other => panic!("拼屏失败: {other:?}"),
    }
    assert!(output.is_file(), "输出文件不存在");
    assert!(std::fs::metadata(&output).unwrap().len() > 0);

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 右机位早开一秒能被自动对齐算出来() {
    let ffmpeg = ngy_video_player::platform::ffmpeg::executable().expect("本机没有 ffmpeg");
    let dir = std::env::temp_dir().join(format!("ngy-multicam-sync-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // 用白噪声而不是正弦：正弦有周期性，相关峰会一排，噪声的峰才是唯一的那一个。
    // 同一个 seed 保证两路拿到的是同一段声音。
    let left = dir.join("left.wav");
    let generated = Command::new(&ffmpeg)
        .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg("anoisesrc=color=white:duration=6:amplitude=0.5:seed=7")
        .arg(&left)
        .status()
        .expect("生成左机位音频失败");
    assert!(generated.success());

    // 右机位前面多一秒静音，相当于早开了一秒。
    let right = dir.join("right.wav");
    let generated = Command::new(&ffmpeg)
        .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg("anullsrc=r=48000:cl=mono:d=1")
        .args(["-f", "lavfi", "-i"])
        .arg("anoisesrc=color=white:duration=6:amplitude=0.5:seed=7")
        .args(["-filter_complex", "[0:a][1:a]concat=n=2:v=0:a=1"])
        .arg(&right)
        .status()
        .expect("生成右机位音频失败");
    assert!(generated.success());

    let alignment = ngy_video_player::multicam::sync::align(&left, &right).expect("对齐失败");
    assert!(
        (alignment.skip - 1.0).abs() < 0.05,
        "偏移不对：{}",
        alignment.skip
    );
    assert!(alignment.confidence > 6.0);

    std::fs::remove_dir_all(&dir).unwrap();
}
