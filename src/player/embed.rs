//! 内嵌在可执行文件里的 libmpv。
//!
//! release 构建时 `build.rs` 把 `libs/<平台>/` 下的库写进二进制，于是分发形态就是
//! 一个 exe：拷到哪台机器上都能播，不必在旁边摆 dll（见 `docs/adr/0005`）。
//!
//! 运行时必须先落盘再加载——`LoadLibrary`/`dlopen` 只认文件，Windows 没有「从内存
//! 加载 DLL」的公开接口。落盘细节与缓存清理见 `crate::embed`。

use anyhow::Result;
use std::path::PathBuf;

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
    let result = crate::embed::materialize_into(&crate::embed::cache_dir("libmpv"), name, bytes);
    if let Ok(path) = &result {
        log_info!("已从可执行文件解压内置 libmpv: {}", path.display());
        let _ = READY.set(path.clone());
    }
    Some(result)
}
