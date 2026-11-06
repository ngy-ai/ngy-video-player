//! 素材库的纯函数：规格文案、封面尺寸、导入进度。面板本身要靠窗口，不在这里测。

use super::covers::{cover_size, fps_label, render_cover_in, spec};
use super::panel::LibraryImport;
use crate::timeline::MediaInfo;

#[test]
fn 规格按有无画面分开写() {
    let video = MediaInfo {
        width: 1920,
        height: 1080,
        fps: Some(crate::timeline::Rational::FPS_30),
        ..MediaInfo::default()
    };
    assert_eq!(spec(&video), "1920×1080 · 30fps");
    assert_eq!(
        fps_label(crate::timeline::Rational::FPS_29_97),
        "29.97",
        "23.976 / 29.97 这类不能四舍五入成整数"
    );

    // 手机竖拍：文件里是横的，显示按旋转后的尺寸。
    let portrait = MediaInfo {
        width: 1920,
        height: 1080,
        rotation: 90,
        ..MediaInfo::default()
    };
    assert_eq!(spec(&portrait), "1080×1920");

    let audio = MediaInfo {
        channels: 2,
        sample_rate: 48_000,
        ..MediaInfo::default()
    };
    assert_eq!(spec(&audio), "48.0kHz · 2 声道");
    assert_eq!(
        spec(&MediaInfo {
            channels: 2,
            sample_rate: 44_100,
            ..MediaInfo::default()
        }),
        "44.1kHz · 2 声道",
        "44.1 不能舍成 44"
    );
    assert_eq!(spec(&MediaInfo::default()), "无可用的音视频轨");
}

#[test]
fn 封面只缩小不放大() {
    assert_eq!(cover_size(160, 90), (160, 90));
    assert_eq!(cover_size(80, 45), (80, 45), "小图原样保留");
    // 竖拍：宽度压到 160，高度按比例跟下来。
    assert_eq!(cover_size(320, 480), (160, 240));
    assert_eq!(cover_size(0, 0), (1, 1), "坏尺寸给个能画的方块");
}

#[test]
fn 导入进度按完成数判定结束() {
    let mut import = LibraryImport {
        total: 2,
        done: 0,
        failed: 0,
    };
    assert!(!import.finished());
    import.done = 1;
    import.failed = 1;
    assert!(!import.finished());
    import.done = 2;
    assert!(import.finished(), "失败也算跑完，不能卡在进度条上");
}

// 需要本机 ffmpeg：`cargo test -- --ignored`。
#[test]
#[ignore = "需要本机 ffmpeg：cargo test -- --ignored"]
fn 真实视频能抽出封面() {
    let dir = std::env::temp_dir().join(format!("ngy-lib-cover-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建临时目录");
    let file = dir.join("clip.mp4");

    let executable = crate::platform::ffmpeg::executable().expect("本机没有 ffmpeg");
    let status = std::process::Command::new(&executable)
        .args([
            "-hide_banner",
            "-nostdin",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=2:size=320x240:rate=25",
            "-c:v",
            "mpeg4",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&file)
        .status()
        .expect("生成测试素材");
    assert!(status.success(), "ffmpeg 生成素材失败");

    let image = render_cover_in(&file, &dir.join("cache"))
        .expect("生成封面")
        .expect("两秒的素材应当有第一帧");
    let size = image.size(0);
    // 320×240 缩到 160 宽，高度按比例。
    assert_eq!((i32::from(size.width), i32::from(size.height)), (160, 120));

    let _ = std::fs::remove_dir_all(&dir);
}
