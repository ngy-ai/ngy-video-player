//! 裁切 / 转换用的 ffmpeg 可执行文件，以及跑子进程时共用的等待逻辑。
//!
//! 裁切与格式转换都要重编码，libmpv 只负责播放、做不了输出。release 构建把
//! `libs/<平台>/ffmpeg{,.exe}` 内嵌进二进制（见 `build.rs`），运行时解压到缓存目录
//! 再以子进程调用——这样分发包仍然是一个 exe，不要求用户自己装 ffmpeg。
//!
//! 查找顺序：`NGY_FFMPEG` → 内嵌 → exe 同目录 → `libs/<平台>/`（开发期）→ `PATH`。
//! 后两档是给 `cargo run` 与「用户本来就有 ffmpeg」的场景留的退路。

use anyhow::{Context as _, Result, anyhow};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

// `build.rs` 生成的表：这次构建内嵌了哪个可执行文件（`NAME`）、内容是什么（`BYTES`）。
include!(concat!(env!("OUT_DIR"), "/ffmpeg_embed.rs"));

/// 拿到可以直接执行的 ffmpeg 路径。
///
/// 成功结果会缓存下来：裁切一次只解析一遍，不必每次都在文件系统里找。
pub fn executable() -> Result<PathBuf> {
    static FOUND: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    FOUND
        .get_or_init(resolve)
        .clone()
        .map_err(|message| anyhow::anyhow!(message))
}

fn resolve() -> Result<PathBuf, String> {
    if let Ok(explicit) = std::env::var("NGY_FFMPEG")
        && !explicit.is_empty()
    {
        let path = PathBuf::from(&explicit);
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!("NGY_FFMPEG 指向的文件不存在：{}", path.display()));
    }

    if let (Some(name), Some(bytes)) = (NAME, BYTES) {
        return crate::embed::materialize_into(&crate::embed::cache_dir("ffmpeg"), name, bytes)
            .map_err(|error| format!("解压内置 ffmpeg 失败：{error:#}"));
    }

    for candidate in local_candidates() {
        if candidate.is_file() {
            log_info!("使用本地 ffmpeg: {}", candidate.display());
            return Ok(candidate);
        }
    }

    if let Some(path) = from_path() {
        log_info!("使用系统 PATH 里的 ffmpeg: {}", path.display());
        return Ok(path);
    }

    Err(
        "没有可用的 ffmpeg：发布版会把 ffmpeg 内嵌进可执行文件；开发时先运行 \
         scripts/fetch-ffmpeg.sh（或用 NGY_FFMPEG 指定可执行文件路径）"
            .to_string(),
    )
}

/// 开发期与本机自带的几种放法，按「离程序越近越可信」排序。
fn local_candidates() -> Vec<PathBuf> {
    let names = file_names();
    let mut dirs: Vec<PathBuf> = Vec::new();

    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        dirs.push(dir.to_path_buf());
        // 开发期：target/debug/../../libs/<platform>
        if let Some(root) = dir.parent().and_then(|path| path.parent()) {
            dirs.push(root.join("libs").join(platform_dir()));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("libs").join(platform_dir()));
        dirs.push(cwd);
    }

    dirs.into_iter()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .collect()
}

/// 系统 PATH 里的 ffmpeg。找不到返回 `None`，交给调用方给出完整的错误说明。
fn from_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let names = file_names();
    for dir in std::env::split_paths(&path) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn file_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["ffmpeg.exe"]
    } else {
        &["ffmpeg"]
    }
}

/// 与 `player/ffi.rs`、`build.rs` 保持一致的平台目录名。
///
/// 三处各写一份是无奈：构建脚本是独立编译的程序，平台层又不能反向依赖播放器。
/// 改目录约定时三处要一起改。
fn platform_dir() -> &'static str {
    if cfg!(windows) {
        if cfg!(target_arch = "aarch64") {
            "windows-arm64"
        } else {
            "windows-x86_64"
        }
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "macos-arm64"
        } else {
            "macos-x86_64"
        }
    } else if cfg!(target_arch = "aarch64") {
        "linux-arm64"
    } else {
        "linux-x86_64"
    }
}

/// 轮询等待子进程退出。
///
/// 不用 `wait()` 是因为它要独占 `&mut Child`，取消时就没法杀进程了；100ms 的
/// 轮询延迟对「点取消」这件事完全够用。子进程退出（或被取消方杀掉）后会把句柄
/// 从槽位里取走，返回它的退出状态；槽位已经空了则返回 `None`。
pub fn wait_polling(child: &Arc<Mutex<Option<Child>>>) -> Option<std::io::Result<ExitStatus>> {
    loop {
        {
            let mut slot = child.lock().unwrap();
            let process = slot.as_mut()?;
            match process.try_wait() {
                Ok(Some(status)) => {
                    slot.take();
                    return Some(Ok(status));
                }
                Ok(None) => {}
                Err(error) => {
                    slot.take();
                    return Some(Err(error));
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// 读一个媒体文件的时长（秒）。
///
/// 没有 ffprobe 可用，直接跑 `ffmpeg -i`：读不到输出文件时它会把 `Duration:` 打在
/// stderr 上并以非零码退出，这里只取那一行。批处理拿它当进度分母，拼接拿它算总进度。
pub fn probe_duration(executable: &Path, input: &Path) -> Result<f64> {
    let output = Command::new(executable)
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(input)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("启动 ffmpeg 探测时长失败：{}", executable.display()))?;
    let text = String::from_utf8_lossy(&output.stderr);
    parse_duration_line(&text).ok_or_else(|| anyhow!("读不出这个文件的时长：{}", input.display()))
}

/// 从 ffmpeg 的输出里找 `Duration: HH:MM:SS.ss`。
///
/// `N/A`（直播流、损坏的文件）返回 `None`：调用方据此退化成「不确定进度」，
/// 而不是拿 0 当分母算出一串无穷大。
pub(crate) fn parse_duration_line(text: &str) -> Option<f64> {
    let line = text
        .lines()
        .find(|line| line.trim_start().starts_with("Duration:"))?;
    let stamp = line
        .trim_start()
        .strip_prefix("Duration:")?
        .split(',')
        .next()?
        .trim();
    let mut parts = stamp.split(':');
    let hours: f64 = parts.next()?.parse().ok()?;
    let minutes: f64 = parts.next()?.parse().ok()?;
    let seconds: f64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(hours * 3600.0 + minutes * 60.0 + seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 时长行能解析也认得_n_a() {
        assert_eq!(
            parse_duration_line("  Duration: 00:01:30.50, start: 0.000000, bitrate: 12 kb/s"),
            Some(90.5)
        );
        assert_eq!(parse_duration_line("  Duration: N/A, bitrate: N/A"), None);
        assert_eq!(parse_duration_line("  Stream #0:0"), None);
        // 多行输出里只认 Duration 那一行。
        assert_eq!(
            parse_duration_line("Input #0\n  Duration: 00:00:02.00, start: 0.0\nStream"),
            Some(2.0)
        );
    }

    #[test]
    fn 平台目录名与构建脚本一致() {
        // 目录名对不上时，开发期找不到 libs/ 里的可执行文件，却不会有任何报错。
        let expected = if cfg!(windows) {
            if cfg!(target_arch = "aarch64") {
                "windows-arm64"
            } else {
                "windows-x86_64"
            }
        } else if cfg!(target_os = "macos") {
            if cfg!(target_arch = "aarch64") {
                "macos-arm64"
            } else {
                "macos-x86_64"
            }
        } else if cfg!(target_arch = "aarch64") {
            "linux-arm64"
        } else {
            "linux-x86_64"
        };
        assert_eq!(platform_dir(), expected);
    }

    #[test]
    fn 可执行文件名带平台后缀() {
        assert_eq!(
            file_names()[0],
            if cfg!(windows) {
                "ffmpeg.exe"
            } else {
                "ffmpeg"
            }
        );
    }
}
