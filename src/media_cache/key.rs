//! 缓存键：一份素材在磁盘缓存里的身份。
//!
//! 键 = 路径哈希 + 文件大小 + 修改时间（纳秒）。文件一改就换一个目录，旧缓存
//! 不会被误用；同一份素材反复拖动时键稳定，能直接命中。
//!
//! 不读文件内容做哈希：几百 MB 的素材每次打开时间线都算一遍，代价远大于收益，
//! 而「大小与修改时间都没变、内容却变了」的情况在这个工具里可以忽略。

use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

/// 素材在缓存里的身份。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MediaKey {
    /// 绝对路径的 FNV-1a 哈希。只用来分目录，不做安全用途。
    hash: u64,
    pub size: u64,
    /// 修改时间：Unix 纳秒。比秒精度更不容易撞上「同一秒内换文件」。
    pub modified: i64,
}

impl MediaKey {
    /// 读文件指纹。文件不存在或读不到元数据时返回 `None`。
    pub fn of(path: &Path) -> Option<Self> {
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let meta = fs::metadata(&absolute).ok()?;
        Some(Self {
            hash: fnv1a(absolute.to_string_lossy().as_bytes()),
            size: meta.len(),
            modified: meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|elapsed| elapsed.as_nanos().min(i64::MAX as u128) as i64)
                .unwrap_or(0),
        })
    }

    /// 目录名：`<16 位哈希>-<大小>-<修改时间>`。参数差异由子目录 / 文件名区分。
    pub fn dir_name(self) -> String {
        format!("{:016x}-{}-{}", self.hash, self.size, self.modified)
    }
}

/// FNV-1a 64 位。稳定、无依赖，换 Rust 版本也不会变——缓存目录名必须可复现。
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ngy-vp-key-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("创建临时目录");
        dir
    }

    #[test]
    fn 同一份文件得到同一个键() {
        let dir = scratch("same");
        let file = dir.join("a.mp4");
        fs::write(&file, b"content").expect("写文件");

        let first = MediaKey::of(&file).expect("读指纹");
        let second = MediaKey::of(&file).expect("读指纹");
        assert_eq!(first, second);
        assert_eq!(first.dir_name(), second.dir_name());
        assert_eq!(
            MediaKey::of(&dir.join("missing.mp4")),
            None,
            "文件不在就没有键"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 文件一改键就变() {
        let dir = scratch("change");
        let file = dir.join("a.mp4");
        fs::write(&file, b"one").expect("写文件");
        let first = MediaKey::of(&file).expect("读指纹");

        fs::write(&file, b"two-two").expect("改文件");
        let second = MediaKey::of(&file).expect("读指纹");
        assert_ne!(first, second);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 目录名是稳定的十六进制() {
        let key = MediaKey {
            hash: 0x1234_5678_9abc_def0,
            size: 42,
            modified: 1_700_000_000_000_000_000,
        };
        assert_eq!(key.dir_name(), "123456789abcdef0-42-1700000000000000000");
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
    }
}
