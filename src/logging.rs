//! 最小日志：终端 + 滚动日志文件。
//!
//! 播放器出问题时（某个格式打不开、某个平台没有声卡）必须能留下现场，而发布版
//! 没有控制台，所以日志文件是唯一线索。这里不引入 tracing 那套生态：一个进程只
//! 有一个日志文件，几行代码就够。

use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

static LOG_FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();
static LOG_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

/// 日志文件位置：跟随系统惯例。
pub fn log_path() -> Option<PathBuf> {
    LOG_PATH.get().cloned().flatten()
}

pub fn init() {
    let path = default_log_path();
    if let Some(path) = &path {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    let file = path.as_ref().and_then(|path| {
        // 超过 8MB 就轮转一次，避免长期使用后日志无限增长。
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.len() > 8 * 1024 * 1024 {
                let old = path.with_extension("log.1");
                let _ = std::fs::rename(path, old);
            }
        }
        OpenOptions::new().create(true).append(true).open(path).ok()
    });

    let _ = LOG_FILE.set(Mutex::new(file));
    let _ = LOG_PATH.set(path);

    let version = env!("CARGO_PKG_VERSION");
    write(
        "INFO",
        format_args!(
            "ngy-video-player {version} 启动（{}）",
            std::env::consts::OS
        ),
    );
}

fn default_log_path() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("NGY_VIDEO_LOG_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir).join("player.log"));
        }
    }

    #[cfg(windows)]
    {
        let base = std::env::var("LOCALAPPDATA")
            .ok()
            .map(PathBuf::from)
            .or_else(|| std::env::var("APPDATA").ok().map(PathBuf::from))?;
        Some(
            base.join("ngy-video-player")
                .join("logs")
                .join("player.log"),
        )
    }

    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").ok()?;
        Some(
            PathBuf::from(home)
                .join("Library")
                .join("Logs")
                .join("ngy-video-player")
                .join("player.log"),
        )
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let base = std::env::var("XDG_STATE_HOME")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|home| PathBuf::from(home).join(".local").join("state"))
            })?;
        Some(base.join("ngy-video-player").join("player.log"))
    }
}

pub fn write(level: &str, args: std::fmt::Arguments<'_>) {
    let message = format!("[{level}] {args}");
    if level == "ERROR" || level == "WARN" || cfg!(debug_assertions) {
        eprintln!("{message}");
    }
    if let Some(file) = LOG_FILE.get() {
        if let Ok(mut guard) = file.lock() {
            if let Some(file) = guard.as_mut() {
                let _ = writeln!(file, "{message}");
                let _ = file.flush();
            }
        }
    }
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => { $crate::logging::write("INFO", format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => { $crate::logging::write("WARN", format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => { $crate::logging::write("ERROR", format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {
        if cfg!(debug_assertions) {
            $crate::logging::write("DEBUG", format_args!($($arg)*));
        }
    };
}
