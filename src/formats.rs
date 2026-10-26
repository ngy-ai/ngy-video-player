//! 格式清单。
//!
//! 播放能力来自 libmpv 内置的 FFmpeg，能解的容器/编码远多于这里列出的扩展名
//! （蓝光 ISO、网络流、各种冷门封装都能吃）。这张表的用途只有两个：
//!
//! * 文件对话框的过滤器；
//! * 拖入文件时判断「这看起来不像媒体」，避免把目录里的 .txt 也排队进去。
//!
//! 判断逻辑刻意宽松：不在表里但 mpv 认识的格式，仍然可以通过命令行参数打开。

/// 视频容器 / 扩展名。
pub const VIDEO: &[&str] = &[
    // 主流封装
    "mp4", "m4v", "mkv", "webm", "mov", "avi", "flv", "f4v", "wmv", "asf",
    "divx", // 传输流与光盘
    "ts", "m2ts", "mts", "m2t", "tp", "trp", "vob", "evo", "ifo", // MPEG 家族
    "mpg", "mpeg", "mpe", "m1v", "m2v", "mpv", "mxf", // 开放容器
    "ogv", "ogg", "ogm", "rm", "rmvb", "ra", "3gp", "3g2", "3gpp", // 专业与冷门
    "dv", "y4m", "nut", "nsv", "amv", "wtv", "dvr-ms", "bik", "smk", "roq", "flc", "fli",
    // 高分辨率
    "h264", "h265", "hevc", "264", "265", "av1",
];

/// 纯音频。
pub const AUDIO: &[&str] = &[
    "mp3", "mp2", "mpa", "flac", "wav", "wave", "aac", "m4a", "alac", "ape", "tta", "wv", "opus",
    "oga", "ogx", "spx", "wma", "ac3", "eac3", "dts", "dtshd", "mka", "aif", "aiff", "aifc", "au",
    "snd", "amr", "dsf", "dff", "caf", "ra", "rm", "mid", "midi",
];

/// mpv 也能当图片查看器用（可缩放、可幻灯片式播放）。
pub const IMAGE: &[&str] = &[
    "jpg", "jpeg", "jpe", "png", "gif", "webp", "bmp", "tif", "tiff",
];

/// 字幕。
pub const SUBTITLE: &[&str] = &[
    "srt", "ass", "ssa", "vtt", "sub", "smi", "sami", "rt", "txt", "ttml", "dfxp", "lrc",
];

/// 播放列表文件。
pub const PLAYLIST: &[&str] = &["m3u", "m3u8", "pls", "xspf", "asx", "cue"];

/// 光盘镜像。
pub const DISC: &[&str] = &["iso", "img", "mds", "mdx", "nrg", "bin"];

pub fn extension(path: &std::path::Path) -> Option<String> {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
}

fn matches(list: &[&str], ext: &str) -> bool {
    list.iter().any(|candidate| *candidate == ext)
}

/// 这个路径看起来是不是可以播放的东西。
pub fn is_media(path: &std::path::Path) -> bool {
    if path.is_dir() {
        return false;
    }
    match extension(path) {
        Some(ext) => {
            matches(VIDEO, &ext)
                || matches(AUDIO, &ext)
                || matches(IMAGE, &ext)
                || matches(DISC, &ext)
        }
        None => false,
    }
}

pub fn is_subtitle(path: &std::path::Path) -> bool {
    extension(path).is_some_and(|ext| matches(SUBTITLE, &ext))
}

pub fn is_playlist(path: &std::path::Path) -> bool {
    extension(path).is_some_and(|ext| matches(PLAYLIST, &ext))
}

pub fn kind_label(path: &std::path::Path) -> &'static str {
    match extension(path) {
        Some(ext) if matches(VIDEO, &ext) => "视频",
        Some(ext) if matches(AUDIO, &ext) => "音频",
        Some(ext) if matches(IMAGE, &ext) => "图片",
        Some(ext) if matches(DISC, &ext) => "光盘",
        _ => "文件",
    }
}

/// 文件对话框能选的全部扩展名。
///
/// 注意这里给的是**裸扩展名**（`mp4`，不是 `*.mp4`）：`rfd::FileDialog::add_filter`
/// 自己会拼上 `*.` 前缀再交给系统。送进来带通配符的字符串会被拼成 `*.*.mp4`，
/// 那是无效的过滤串，Windows 会一个文件都不显示——「对话框里看不到 mp4」就是这么来的。
pub fn dialog_extensions() -> Vec<&'static str> {
    VIDEO
        .iter()
        .chain(AUDIO)
        .chain(IMAGE)
        .chain(DISC)
        .copied()
        .collect()
}

/// 「所有支持的文件」过滤项：(显示名, 裸扩展名列表)。
pub fn dialog_filter() -> (String, Vec<&'static str>) {
    ("所有支持的媒体文件".to_string(), dialog_extensions())
}

/// 给「关于/设置」页展示的计数。
pub fn supported_extension_count() -> usize {
    VIDEO.len() + AUDIO.len() + IMAGE.len() + DISC.len()
}
