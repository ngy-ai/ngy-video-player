//! 示波器抽帧的端到端验证：需要本机 ffmpeg，默认忽略。

use ngy_video_player::scope::{BINS, sample};
use std::process::Command;

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 从真实视频抽一帧统计三种示波器() {
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
    let luma = &scope.luma;
    assert_eq!(luma.total, 160 * 90);
    assert_eq!(luma.bins.iter().sum::<u32>(), luma.total);
    assert_eq!(luma.bins.len(), BINS);
    // testsrc2 是彩色测试图，不可能全黑或全白。
    assert!(luma.black_ratio() < 0.5);
    assert!(luma.mean() > 0.05);

    // 彩条在波形图与矢量图上都要留下痕迹，否则说明抽帧格式或统计写错了。
    assert!(scope.waveform_peak() > 0, "波形图是空的");
    assert!(scope.vectorscope_peak() > 0, "矢量图是空的");
    assert_eq!(
        scope.waveform_bitmap().pixels.len(),
        (320 * 128 * 4) as usize
    );
    assert_eq!(
        scope.vectorscope_bitmap().pixels.len(),
        (128 * 128 * 4) as usize
    );

    std::fs::remove_dir_all(&dir).unwrap();
}
