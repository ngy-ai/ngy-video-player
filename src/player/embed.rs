//! 内嵌在可执行文件里的 libmpv。
//!
//! release 构建时 `build.rs` 把 `libs/<平台>/` 下的库写进二进制，于是分发形态就是
//! 一个 exe：拷到哪台机器上都能播，不必在旁边摆 dll（见 `docs/adr/0005`）。
//!
//! 运行时必须先落盘再加载——`LoadLibrary`/`dlopen` 只认文件，Windows 没有「从内存
//! 加载 DLL」的公开接口。缓存文件名带内容哈希：换一版库会写出新文件，不会误用旧的，
//! 旧文件在下次成功解压后清掉。

use anyhow::{Context as _, Result};
use std::path::{Path, PathBuf};

// `build.rs` 生成的表：这次构建内嵌了哪个库（`NAME`）、内容是什么（`BYTES`）。
include!(concat!(env!("OUT_DIR"), "/mpv_embed.rs"));

/// 把内置库落到用户缓存目录，返回可以直接交给动态加载器的文件路径。
///
/// - `None`：这次构建没有内嵌库（debug 构建，或 release 时 `libs/` 里没有库）；
/// - `Some(Err)`：内嵌了，但落盘失败（磁盘满、缓存目录不可写）。
///
/// 成功结果会缓存下来：同一进程里第二次加载（`probe` 探测格式）不必再把上百 MB
/// 的内容哈希一遍。
pub fn materialize() -> Option<Result<PathBuf>> {
    let (name, bytes) = match (NAME, BYTES) {
        (Some(name), Some(bytes)) => (name, bytes),
        _ => return None,
    };

    static READY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    if let Some(path) = READY.get() {
        return Some(Ok(path.clone()));
    }

    // 具体失败现场（哪个目录、哪个文件）由 `materialize_into` 自己带上。
    let result = materialize_into(&cache_dir(), name, bytes);
    if let Ok(path) = &result {
        log_info!("已从可执行文件解压内置 libmpv: {}", path.display());
        let _ = READY.set(path.clone());
    }
    Some(result)
}

/// 内置库放在哪：跟随系统惯例的用户缓存目录，卸载程序时用户能整目录删掉。
fn cache_dir() -> PathBuf {
    base_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("ngy-video-player")
        .join("libmpv")
}

fn base_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("APPDATA"))
            .map(PathBuf::from)
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library").join("Caches"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
    }
    #[cfg(not(any(windows, unix)))]
    {
        None
    }
}

fn materialize_into(dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("创建缓存目录 {} 失败", dir.display()))?;

    let file = cache_name(name, bytes);
    let target = dir.join(&file);

    // 尺寸对得上就认为上次已经写完整（写入走的是「临时文件 + 改名」，不会有半截文件）。
    if std::fs::metadata(&target).is_ok_and(|meta| meta.len() == bytes.len() as u64) {
        return Ok(target);
    }
    // 尺寸对不上的残文件先删掉：Windows 的 rename 不会覆盖已存在的目标。
    let _ = std::fs::remove_file(&target);

    // 先写临时文件再改名：中途被打断不会留下半个 DLL；另一个实例同时在解压时，
    // 谁先改名成功算谁的，输的一方直接用赢家的文件。
    let temp = dir.join(format!(".{}-{file}.tmp", std::process::id()));
    std::fs::write(&temp, bytes).with_context(|| format!("写入 {} 失败", temp.display()))?;

    match std::fs::rename(&temp, &target) {
        Ok(()) => {}
        Err(_) if target.is_file() => {
            let _ = std::fs::remove_file(&temp);
        }
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            return Err(anyhow::Error::from(error).context(format!(
                "把 {} 改名为 {} 失败",
                temp.display(),
                target.display()
            )));
        }
    }

    remove_stale(dir, &file);
    Ok(target)
}

/// 缓存文件名 = 内容哈希 + 原始库名，例如 `1f0a…-libmpv-2.dll`。
///
/// 保留库名是为了留住扩展名：加载器按扩展名判断怎么打开，Windows 上没有 `.dll`
/// 后缀的文件会被当成数据文件。
fn cache_name(name: &str, bytes: &[u8]) -> String {
    format!("{:016x}-{name}", checksum(bytes))
}

/// 内容校验和，只为回答「这份文件和缓存里那份是不是同一份」。
///
/// 不是防篡改，用不着引一个哈希库：按 8 字节块混入的 FNV-1a 已经够用，而且
/// 120MB 的库扫一遍是内存带宽的量级，逐字节的串行依赖要多花几百毫秒。
fn checksum(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for chunk in bytes.chunks(8) {
        let mut word = [0u8; 8];
        word[..chunk.len()].copy_from_slice(chunk);
        hash ^= u64::from_le_bytes(word);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash ^ (bytes.len() as u64)
}

/// 清掉旧构建解压出来的库文件（`<哈希>-<库名>`）。
///
/// 失败不管：文件可能正被另一个实例用着，留着只是多占点磁盘。
fn remove_stale(dir: &Path, keep: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name != keep && is_cache_file(&name) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// 是不是本模块写出来的库文件：`<16 位十六进制哈希>-<库名>`。
/// 临时文件以 `.` 开头、以 `.tmp` 结尾，不会落进这个形状里。
fn is_cache_file(name: &str) -> bool {
    name.split_once('-').is_some_and(|(hash, rest)| {
        !rest.is_empty() && hash.len() == 16 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ngy-embed-{}-{tag}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn cache_name_keeps_extension_and_tracks_content() {
        let a = cache_name("libmpv-2.dll", b"aaa");
        assert!(a.ends_with("-libmpv-2.dll"), "{a}");
        assert_ne!(a, cache_name("libmpv-2.dll", b"bbb"));
        // 同样内容必须给出同样的名字，否则每次启动都要再写一份上百 MB 的文件。
        assert_eq!(a, cache_name("libmpv-2.dll", b"aaa"));
    }

    #[test]
    fn materialize_reuses_the_file_it_wrote() {
        let dir = scratch("reuse");
        let first = materialize_into(&dir, "libmpv-2.dll", b"payload").unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), b"payload");

        let second = materialize_into(&dir, "libmpv-2.dll", b"payload").unwrap();
        assert_eq!(first, second);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn truncated_cache_file_is_written_again() {
        let dir = scratch("truncated");
        let path = materialize_into(&dir, "libmpv-2.dll", b"payload").unwrap();
        std::fs::write(&path, b"pa").unwrap();

        let again = materialize_into(&dir, "libmpv-2.dll", b"payload").unwrap();
        assert_eq!(std::fs::read(&again).unwrap(), b"payload");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn libraries_from_older_builds_are_removed() {
        let dir = scratch("stale");
        let old = materialize_into(&dir, "libmpv-2.dll", b"old").unwrap();
        let new = materialize_into(&dir, "libmpv-2.dll", b"new").unwrap();

        assert_ne!(old, new);
        assert!(!old.exists(), "旧版本的库应当被清掉");
        assert!(new.exists());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn temp_files_are_not_mistaken_for_libraries() {
        assert!(is_cache_file(&cache_name("libmpv-2.dll", b"x")));
        assert!(!is_cache_file(".4242-0123456789abcdef-libmpv-2.dll.tmp"));
        assert!(!is_cache_file("readme.txt"));
    }
}
