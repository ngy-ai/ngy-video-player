//! 直方图抽帧的端到端验证：需要本机 ffmpeg，默认忽略。

use ngy_video_player::scope::{BINS, sample};
use std::process::Command;

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 从真实视频抽一帧统计亮度() {
    let ffmpeg = ngy_video_player::platform::ffmpeg::executable().expect("本机没有 ffmpeg");
    let dir = std::env::temp_dir().join(format!("ngy-scope-e2e-{}", std::process::id()));
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
            "testsrc2=size=320x180:rate=25:duration=2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&input)
        .status()
        .expect("生成测试视频失败");
    assert!(generated.success());

    let scope = sample(&input, 1.0).expect("抽帧失败");
    assert_eq!(scope.total, 160 * 90);
    assert_eq!(scope.bins.iter().sum::<u32>(), scope.total);
    assert_eq!(scope.bins.len(), BINS);
    // testsrc2 是彩色测试图，不可能全黑或全白。
    assert!(scope.black_ratio() < 0.5);
    assert!(scope.mean() > 0.05);

    std::fs::remove_dir_all(&dir).unwrap();
}
