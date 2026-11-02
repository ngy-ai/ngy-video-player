//! 字幕编码选择：把字幕搬进 mkv 时用什么编码。
//!
//! 点阵字幕（PGS/DVD/DVB）只能原样复制；ASS 要留住样式；其余文本字幕统一转成
//! srt——尤其 mp4 的 mov_text 在 mkv 里根本不存在，原样复制会让整个任务在最后
//! 一步失败。轨道编辑、元数据编辑都要这套判断，所以放在这里共用。

use std::path::Path;

/// 「保留原字幕」时的输出编码。
pub(crate) fn kept(codecs: &[String]) -> &'static str {
    let lower: Vec<String> = codecs.iter().map(|codec| codec.to_lowercase()).collect();
    if lower.iter().any(|codec| is_bitmap(codec)) {
        "copy"
    } else if lower.iter().any(|codec| is_ass(codec)) {
        "ass"
    } else {
        "srt"
    }
}

/// 换进来的字幕文件按扩展名选编码：ASS/SSA 保住样式，`.sup` 是点阵图形只能复制，
/// 其余文本格式统一转成 srt。
pub(crate) fn from_path(path: &Path) -> &'static str {
    match crate::formats::extension(path).as_deref() {
        Some("ass" | "ssa") => "ass",
        Some("sup") => "copy",
        _ => "srt",
    }
}

fn is_bitmap(codec: &str) -> bool {
    ["pgs", "dvd_sub", "dvb_sub", "xsub"]
        .iter()
        .any(|key| codec.contains(key))
}

fn is_ass(codec: &str) -> bool {
    codec.contains("ass") || codec.contains("ssa")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept_for(list: &[&str]) -> &'static str {
        let codecs: Vec<String> = list.iter().map(|codec| codec.to_string()).collect();
        kept(&codecs)
    }

    #[test]
    fn 保留字幕按编码决定复制还是转码() {
        assert_eq!(kept_for(&[]), "srt");
        assert_eq!(kept_for(&["subrip"]), "srt");
        // mp4 的 mov_text 装不进 mkv，必须转码。
        assert_eq!(kept_for(&["mov_text"]), "srt");
        assert_eq!(kept_for(&["ass"]), "ass");
        assert_eq!(kept_for(&["hdmv_pgs_subtitle"]), "copy");
        assert_eq!(kept_for(&["dvd_subtitle"]), "copy");
        // 混合时以点阵为准：转成文字会把图形字幕整条丢掉。
        assert_eq!(kept_for(&["subrip", "hdmv_pgs_subtitle"]), "copy");
    }

    #[test]
    fn 替换字幕按扩展名选编码() {
        for (name, codec) in [
            ("sub.srt", "srt"),
            ("sub.vtt", "srt"),
            ("sub.ass", "ass"),
            ("sub.ssa", "ass"),
            ("sub.sup", "copy"),
        ] {
            assert_eq!(from_path(Path::new(name)), codec, "{name}");
        }
    }
}
