//! 时间线的媒体缓存：缩略图条与音频波形。
//!
//! 两者都按「懒加载 + 分块 + 磁盘缓存」设计：
//!
//! - 缩略图每 100 秒一块（10×10 的格子图），滚到哪一块才生成哪一块；
//! - 波形边解码边聚合峰值，内存里只留「每秒 100 个块」，与素材长度无关；
//! - 磁盘键 = 路径哈希 + 大小 + 修改时间，文件一改就换目录，旧缓存不会被误用；
//! - 波形在内存里另有一层 LRU，容量由调用方配置。
//!
//! 目录约定：
//!
//! ```text
//! <缓存根>/<key>/thumbs-v1-medium/strip-0000.jpg
//! <缓存根>/<key>/peaks-v1-100.bin
//! ```
//!
//! 缓存根默认在配置目录下的 `cache/`，可以用 `NGY_VIDEO_CACHE_DIR` 改位置。
//! 清理缓存就是删掉这个目录，程序不需要知道它曾经有什么。

mod key;
mod lru;
mod thumbnail;
mod waveform;

pub use thumbnail::{
    COLUMNS, ROWS, SECONDS_PER_STRIP, ThumbLevel, ThumbnailStrip, ensure_strip, strip_index_of,
    strip_path,
};
pub use waveform::{DEFAULT_CAPACITY, PEAKS_PER_SECOND, WaveformCache, aggregate};

use std::path::PathBuf;

/// 缓存根目录：`NGY_VIDEO_CACHE_DIR` → 配置目录下的 `cache/` → 临时目录。
pub fn cache_root() -> PathBuf {
    if let Ok(dir) = std::env::var("NGY_VIDEO_CACHE_DIR")
        && !dir.is_empty()
    {
        return PathBuf::from(dir);
    }
    crate::settings::config_path()
        .and_then(|path| path.parent().map(|dir| dir.join("cache")))
        .unwrap_or_else(|| std::env::temp_dir().join("ngy-video-player-cache"))
}
