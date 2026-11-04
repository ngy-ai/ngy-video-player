//! 时间线缩略图：按秒抽帧，10×10 拼成一张图块，按 100 秒分块懒生成。
//!
//! 为什么按块而不是整条素材一次抽完：一小时 4K 素材整条解码要几分钟，用户只是
//! 想看到当前视口那几秒长什么样。界面滚到哪一块就生成哪一块，生成完落盘，下次
//! 直接读文件。
//!
//! 每一块用 `-ss` 快速定位，只解这 100 秒，不会碰素材其余部分。

use super::key::MediaKey;
use crate::platform::ffmpeg;
use anyhow::{Context as _, Result, anyhow, bail};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 一张图块覆盖多少秒；10×10 的格子正好每秒一格。
pub const SECONDS_PER_STRIP: u64 = 100;
pub const COLUMNS: u32 = 10;
pub const ROWS: u32 = 10;

/// 生成参数变了就 +1，旧缓存自然失效，不会被当成同一种图块复用。
const CACHE_VERSION: u32 = 1;

/// 时间线缩放档位。放大时用更清晰的档，缩得很小时用最省内存的一档。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ThumbLevel {
    Small,
    Medium,
    Large,
}

impl ThumbLevel {
    pub const ALL: [ThumbLevel; 3] = [ThumbLevel::Small, ThumbLevel::Medium, ThumbLevel::Large];

    /// 每个格子的宽度（像素）。高度跟着素材比例走，不统一拉成 16:9。
    pub fn width(self) -> u32 {
        match self {
            ThumbLevel::Small => 80,
            ThumbLevel::Medium => 160,
            ThumbLevel::Large => 320,
        }
    }

    /// 缓存目录名的一部分。
    pub fn name(self) -> &'static str {
        match self {
            ThumbLevel::Small => "small",
            ThumbLevel::Medium => "medium",
            ThumbLevel::Large => "large",
        }
    }
}

/// 一块已经生成好的缩略图：10×10 的格子图，覆盖 `[index*100, index*100+100)` 秒。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbnailStrip {
    pub path: PathBuf,
    pub index: u64,
    pub columns: u32,
    pub rows: u32,
}

impl ThumbnailStrip {
    /// `second` 在这一块里落在第几列第几行；不在本块范围内返回 `None`。
    pub fn tile(&self, second: u64) -> Option<(u32, u32)> {
        let start = self.index * SECONDS_PER_STRIP;
        if second < start || second >= start + SECONDS_PER_STRIP {
            return None;
        }
        let within = (second - start) as u32;
        let (column, row) = (within % COLUMNS, within / COLUMNS);
        (column < self.columns && row < self.rows).then_some((column, row))
    }
}

/// 第 `second` 秒属于第几块。
pub fn strip_index_of(second: u64) -> u64 {
    second / SECONDS_PER_STRIP
}

/// 图块在缓存里的路径（不保证已生成）。
pub fn strip_path(
    input: &Path,
    level: ThumbLevel,
    cache_root: &Path,
    strip: u64,
) -> Result<PathBuf> {
    let key = MediaKey::of(input).ok_or_else(|| anyhow!("读不到素材信息：{}", input.display()))?;
    Ok(strip_path_in(cache_root, key, level, strip))
}

fn strip_path_in(root: &Path, key: MediaKey, level: ThumbLevel, strip: u64) -> PathBuf {
    root.join(key.dir_name())
        .join(format!("thumbs-v{CACHE_VERSION}-{}", level.name()))
        .join(format!("strip-{strip:04}.jpg"))
}

/// 确保第 `strip` 块已生成。已经超出片尾时返回 `Ok(None)`——没有画面不是错误。
///
/// 缓存命中时不会启动 ffmpeg，滚动时间线才不会因为读缓存而卡。
pub fn ensure_strip(
    input: &Path,
    level: ThumbLevel,
    cache_root: &Path,
    strip: u64,
) -> Result<Option<ThumbnailStrip>> {
    let key = MediaKey::of(input).ok_or_else(|| anyhow!("读不到素材信息：{}", input.display()))?;
    let path = strip_path_in(cache_root, key, level, strip);
    if is_usable(&path) {
        return Ok(Some(strip_of(path, strip)));
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建缩略图缓存目录失败：{}", parent.display()))?;
    }

    let executable = ffmpeg::executable()?;
    let start = strip * SECONDS_PER_STRIP;
    let filter = format!("fps=1,scale={}:-2,tile={COLUMNS}x{ROWS}", level.width());
    let output = Command::new(&executable)
        .args(["-hide_banner", "-nostdin", "-v", "error", "-y"])
        .arg("-ss")
        .arg(start.to_string())
        .arg("-t")
        .arg(SECONDS_PER_STRIP.to_string())
        .arg("-i")
        .arg(input)
        .args([
            "-vf",
            &filter,
            "-frames:v",
            "1",
            "-q:v",
            "5",
            "-update",
            "1",
        ])
        .arg(&path)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("启动 ffmpeg 生成缩略图失败：{}", executable.display()))?;

    if !output.status.success() && !is_usable(&path) {
        let message = String::from_utf8_lossy(&output.stderr);
        // 超出片尾时 ffmpeg 以「没有写出任何东西」告终，这不是失败。
        if message.contains("Output file is empty") || message.contains("Nothing was written") {
            return Ok(None);
        }
        bail!(
            "生成缩略图失败：{}（第 {strip} 块）：{}",
            input.display(),
            message.trim()
        );
    }

    if !is_usable(&path) {
        return Ok(None);
    }
    Ok(Some(strip_of(path, strip)))
}

fn is_usable(path: &Path) -> bool {
    fs::metadata(path)
        .map(|meta| meta.is_file() && meta.len() > 0)
        .unwrap_or(false)
}

fn strip_of(path: PathBuf, index: u64) -> ThumbnailStrip {
    ThumbnailStrip {
        path,
        index,
        columns: COLUMNS,
        rows: ROWS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ngy-vp-thumb-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("创建临时目录");
        dir
    }

    #[test]
    fn 档位越清晰宽度越大() {
        assert!(ThumbLevel::Small.width() < ThumbLevel::Medium.width());
        assert!(ThumbLevel::Medium.width() < ThumbLevel::Large.width());
        let names: Vec<&str> = ThumbLevel::ALL.iter().map(|level| level.name()).collect();
        assert_eq!(names, ["small", "medium", "large"]);
    }

    #[test]
    fn 块内位置按十列排() {
        let strip = ThumbnailStrip {
            path: PathBuf::from("x.jpg"),
            index: 1,
            columns: COLUMNS,
            rows: ROWS,
        };
        assert_eq!(strip.tile(100), Some((0, 0)));
        assert_eq!(strip.tile(123), Some((3, 2)));
        assert_eq!(strip.tile(199), Some((9, 9)));
        assert_eq!(strip.tile(99), None, "上一块的事");
        assert_eq!(strip.tile(200), None, "下一块的事");
        assert_eq!(strip_index_of(0), 0);
        assert_eq!(strip_index_of(199), 1);
    }

    #[test]
    fn 缓存命中时不启动_ffmpeg() {
        let dir = scratch("hit");
        let input = dir.join("a.mp4");
        fs::write(&input, b"fake video").expect("写文件");

        let cache = dir.join("cache");
        let path = strip_path(&input, ThumbLevel::Medium, &cache, 0).expect("算路径");
        fs::create_dir_all(path.parent().unwrap()).expect("建目录");
        fs::write(&path, b"jpeg bytes").expect("写假图块");

        let strip = ensure_strip(&input, ThumbLevel::Medium, &cache, 0)
            .expect("缓存命中不该需要 ffmpeg")
            .expect("图块存在");
        assert_eq!(strip.path, path);
        assert_eq!(strip.index, 0);
        assert_eq!(
            fs::read(&path).unwrap(),
            b"jpeg bytes",
            "命中时不该覆盖已有缓存"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    // 需要本机 ffmpeg：`cargo test -- --ignored`。
    #[test]
    #[ignore = "需要本机 ffmpeg：cargo test -- --ignored"]
    fn 真实视频生成图块并区分片尾() {
        let dir = scratch("real");
        let input = dir.join("clip.mp4");
        let executable = ffmpeg::executable().expect("本机没有 ffmpeg");
        let status = Command::new(&executable)
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
            .arg(&input)
            .status()
            .expect("生成测试素材");
        assert!(status.success(), "ffmpeg 生成素材失败");

        let cache = dir.join("cache");
        let strip = ensure_strip(&input, ThumbLevel::Small, &cache, 0)
            .expect("生成缩略图")
            .expect("第 0 块应当有画面");
        assert!(strip.path.is_file());
        assert!(fs::metadata(&strip.path).unwrap().len() > 0);

        assert!(
            ensure_strip(&input, ThumbLevel::Small, &cache, 5)
                .expect("片尾之后不是错误")
                .is_none(),
            "2 秒的片子没有第 5 块"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
