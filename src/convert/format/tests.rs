use super::*;
use crate::formats;

#[test]
fn 扩展名互不重复() {
    let mut extensions: Vec<&str> = TargetFormat::ALL
        .iter()
        .map(|format| format.extension)
        .collect();
    let total = extensions.len();
    extensions.sort_unstable();
    extensions.dedup();
    assert_eq!(extensions.len(), total, "同一时刻只能有一个格式用这个后缀");
}

#[test]
fn 覆盖了现有的全部视频与音频扩展名() {
    // 目标表只能比播放表窄，不能凭空多出播放器都认不出的后缀。
    for format in TargetFormat::ALL {
        let known = formats::VIDEO.contains(&format.extension)
            || formats::AUDIO.contains(&format.extension);
        assert!(known, "{} 不在 formats 的清单里", format.extension);
    }
    // 反过来：能播又是常见容器的，基本都该能转。这里只卡住几张主流格式，
    // 冷门容器里确实有不支持写出的（见模块文档）。
    for extension in [
        "mp4", "mkv", "mov", "webm", "avi", "ts", "mp3", "flac", "wav", "m4a",
    ] {
        assert!(
            TargetFormat::by_extension(extension).is_some(),
            "缺少常用格式 {extension}"
        );
    }
}

#[test]
fn 每个格式都有说明和编码参数() {
    for format in TargetFormat::ALL {
        assert!(!format.hint().is_empty(), "{}", format.extension);
        match format.kind() {
            TargetKind::Video => {
                assert!(
                    !format.video_args().is_empty(),
                    "{} 是视频容器却没有视频参数",
                    format.extension
                );
            }
            TargetKind::Audio => {
                assert!(
                    format.video_args().is_empty(),
                    "{} 是音频容器不该有视频参数",
                    format.extension
                );
                assert!(
                    !format.audio_args().is_empty(),
                    "{} 没有音频参数",
                    format.extension
                );
            }
        }
        assert!(!format.muxer().is_empty(), "{}", format.extension);
    }
}

#[test]
fn 只有_mkv_保留字幕() {
    for format in TargetFormat::ALL {
        let subtitles = format.copy_subtitles();
        assert_eq!(subtitles, format.extension == "mkv", "{}", format.extension);
    }
}

#[test]
fn 默认是_mp4() {
    assert_eq!(TargetFormat::default().extension(), "mp4");
    assert_eq!(TargetFormat::by_extension("nope"), None);
}

#[test]
fn 没有探测到硬编时用_x264_参数() {
    let mp4 = TargetFormat::by_extension("mp4").unwrap();
    let args = mp4.video_args_with(None, None);
    assert!(args.windows(2).any(|pair| pair == ["-c:v", "libx264"]));
    assert!(args.windows(2).any(|pair| pair == ["-crf", "20"]));
    assert!(
        args.windows(2)
            .any(|pair| pair == ["-movflags", "+faststart"]),
        "{args:?}"
    );
}

#[test]
fn 探测到硬编就换成显卡并给码率() {
    let mp4 = TargetFormat::by_extension("mp4").unwrap();
    let args = mp4.video_args_with(Some("h264_nvenc"), None);
    assert!(
        args.windows(2).any(|pair| pair == ["-c:v", "h264_nvenc"]),
        "{args:?}"
    );
    // 硬件编码器不认 CRF，必须换成码率。
    assert!(!args.iter().any(|arg| arg == "-crf"), "{args:?}");
    assert!(args.windows(2).any(|pair| pair == ["-b:v", "10M"]));
    assert!(args.windows(2).any(|pair| pair == ["-pix_fmt", "yuv420p"]));
    // 索引位置与编码器无关，硬编也要保留。
    assert!(
        args.windows(2)
            .any(|pair| pair == ["-movflags", "+faststart"])
    );

    // HEVC 目标只认 HEVC 的编码器，不能把 H.264 的硬编塞进去。
    let hevc = TargetFormat::by_extension("h265").unwrap();
    let software = hevc.video_args_with(Some("h264_nvenc"), None);
    assert!(software.windows(2).any(|pair| pair == ["-c:v", "libx265"]));
    let hardware = hevc.video_args_with(None, Some("hevc_nvenc"));
    assert!(
        hardware
            .windows(2)
            .any(|pair| pair == ["-c:v", "hevc_nvenc"])
    );
}

#[test]
fn 老设备档位保留_baseline_且不换硬编() {
    let threegp = TargetFormat::by_extension("3gp").unwrap();
    let args = threegp.video_args_with(Some("h264_nvenc"), None);
    assert!(args.windows(2).any(|pair| pair == ["-c:v", "libx264"]));
    assert!(
        args.windows(2)
            .any(|pair| pair == ["-profile:v", "baseline"]),
        "{args:?}"
    );
}

#[test]
fn 固定编码器与音频目标不受探测影响() {
    let webm = TargetFormat::by_extension("webm").unwrap();
    assert_eq!(
        webm.video_args_with(Some("h264_nvenc"), Some("hevc_nvenc")),
        webm.video_args_with(None, None),
        "VP9 这类固定档位不能被换成显卡编码器"
    );

    let mp3 = TargetFormat::by_extension("mp3").unwrap();
    assert!(mp3.video_args_with(Some("h264_nvenc"), None).is_empty());
}

#[test]
fn 搜索能按扩展名_编码和分类命中() {
    // 扩展名。
    assert!(TargetFormat::by_extension("mkv").unwrap().matches("mkv"));
    assert!(TargetFormat::by_extension("mkv").unwrap().matches("MK"));
    // 编码说明：想找 h264 的人不用先知道容器叫什么。
    let h264: Vec<&str> = TargetFormat::ALL
        .iter()
        .filter(|format| format.matches("h264") || format.matches("H.264"))
        .map(|format| format.extension)
        .collect();
    assert!(h264.contains(&"mp4") && h264.contains(&"flv"), "{h264:?}");
    // 无损音频。
    let lossless: Vec<&str> = TargetFormat::ALL
        .iter()
        .filter(|format| format.matches("无损"))
        .map(|format| format.extension)
        .collect();
    assert_eq!(lossless, vec!["flac", "mka", "alac", "wv"], "{lossless:?}");
    // 分类。
    assert!(TargetFormat::by_extension("mp3").unwrap().matches("音频"));
    assert!(!TargetFormat::by_extension("mp3").unwrap().matches("视频"));
    // 空搜索词命中全部。
    assert!(TargetFormat::ALL.iter().all(|format| format.matches("  ")));
    // 搜不到就是空。
    assert!(!TargetFormat::by_extension("mp3").unwrap().matches("zzz"));
}
