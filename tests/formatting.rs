//! 时间格式化与格式清单。

use ngy_video_player::formats;
use ngy_video_player::ui::{format_time, format_time_precise};
use std::path::Path;

#[test]
fn 时间格式按长度切换() {
    assert_eq!(format_time(0.0), "0:00");
    assert_eq!(format_time(5.4), "0:05");
    assert_eq!(format_time(59.6), "1:00");
    assert_eq!(format_time(125.0), "2:05");
    assert_eq!(format_time(3600.0), "1:00:00");
    assert_eq!(format_time(4832.5), "1:20:33");
}

#[test]
fn 负数表示剩余时间() {
    assert_eq!(format_time(-65.0), "-1:05");
    assert_eq!(format_time(-0.4), "-0:00");
}

#[test]
fn 非法时长不显示成巨数() {
    assert_eq!(format_time(f64::NAN), "--:--");
    assert_eq!(format_time(f64::INFINITY), "--:--");
}

#[test]
fn 精确时间显示到百分秒() {
    assert_eq!(format_time_precise(0.0), "0:00.00");
    assert_eq!(format_time_precise(8.0), "0:08.00");
    assert_eq!(format_time_precise(8.006), "0:08.01");
    assert_eq!(format_time_precise(59.999), "1:00.00");
    assert_eq!(format_time_precise(3600.25), "1:00:00.25");
    assert_eq!(format_time_precise(-1.5), "-0:01.50");
}

#[test]
fn 精确时间遇到非法值不显示成巨数() {
    assert_eq!(format_time_precise(f64::NAN), "--:--.--");
    assert_eq!(format_time_precise(f64::INFINITY), "--:--.--");
}

#[test]
fn 媒体判定覆盖常见容器与音频() {
    for name in [
        "a.mp4", "b.MKV", "c.avi", "d.webm", "e.m2ts", "f.rmvb", "g.iso", "h.flac", "i.ape",
        "j.opus",
    ] {
        assert!(formats::is_media(Path::new(name)), "{name} 应被识别为媒体");
    }

    for name in ["a.txt", "b.srt", "c.exe", "d"] {
        assert!(
            !formats::is_media(Path::new(name)),
            "{name} 不应被识别为媒体"
        );
    }
}

#[test]
fn 字幕与播放列表判定() {
    assert!(formats::is_subtitle(Path::new("movie.zh.srt")));
    assert!(formats::is_subtitle(Path::new("movie.ass")));
    assert!(!formats::is_subtitle(Path::new("movie.mp4")));

    assert!(formats::is_playlist(Path::new("list.m3u")));
    assert!(formats::is_playlist(Path::new("list.m3u8")));
    assert!(!formats::is_playlist(Path::new("list.mp4")));
}

#[test]
fn 可关联格式只含视频清单与工程() {
    let extensions = formats::associable_extensions();

    // 视频 54 个 + 清单 6 个 + 工程 1 个，三组互不重叠。
    assert_eq!(
        extensions.len(),
        formats::VIDEO.len() + formats::PLAYLIST.len() + formats::PROJECT.len()
    );

    for extension in &extensions {
        assert!(
            !extension.contains('*') && !extension.contains('.'),
            "注册用的扩展名必须是裸的：{extension}"
        );
        assert!(
            formats::VIDEO.contains(extension)
                || formats::PLAYLIST.contains(extension)
                || formats::PROJECT.contains(extension),
            "不该混进非视频格式：{extension}"
        );
    }

    assert!(extensions.contains(&"mp4"));
    assert!(extensions.contains(&"m3u"));
    assert!(
        extensions.contains(&"ngyproj"),
        "双击工程文件要能打开本程序"
    );
    // 抢图片查看器、虚拟光驱和字幕的默认地位不属于播放器的职责。
    assert!(!extensions.contains(&"jpg"));
    assert!(!extensions.contains(&"iso"));
    assert!(!extensions.contains(&"srt"));
    assert!(!extensions.contains(&"mp3"));
}

#[test]
fn 对话框过滤器覆盖全部扩展名() {
    let (label, extensions) = formats::dialog_filter();
    assert!(!label.is_empty());
    assert_eq!(extensions.len(), formats::supported_extension_count());
    assert!(extensions.contains(&"mp4"));
    assert!(extensions.contains(&"flac"));
    // 交给 rfd 的必须是裸扩展名：`rfd` 会自己补 `*.`，带通配符会被拼成
    // `*.*.mp4`，装进系统对话框后一个文件都不显示。
    assert!(
        extensions
            .iter()
            .all(|ext| !ext.contains('*') && !ext.contains('.')),
        "对话框过滤器里不应该带通配符：{extensions:?}"
    );
}
