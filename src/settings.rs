//! 偏好设置：音量、循环模式、面板开合，以及上次打开的目录。
//!
//! 只存「用户改过就希望记住」的东西。播放位置之类不做记忆——大多数场景下
//! 用户并不希望下次打开时从上次中断的地方接着放。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::player::LoopMode;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub volume: f64,
    pub muted: bool,
    pub speed: f64,
    pub loop_mode: LoopMode,
    pub show_playlist: bool,
    pub always_on_top: bool,
    pub last_dir: Option<PathBuf>,
    pub window_width: f32,
    pub window_height: f32,
    pub fullscreen: bool,
    /// 硬件解码：`auto-safe` / `no` / `auto-copy`。
    pub hwdec: String,
    /// 启动时是否自动维护文件关联（把注册项按当前 exe 路径重写一遍）。
    ///
    /// 记的是**用户意图**，不是「当前有没有注册」——后者每次现场问操作系统，
    /// 本地缓存一份只会漂移。
    pub maintain_associations: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            volume: 80.0,
            muted: false,
            speed: 1.0,
            loop_mode: LoopMode::Off,
            show_playlist: false,
            always_on_top: false,
            last_dir: None,
            window_width: 1120.0,
            window_height: 680.0,
            fullscreen: false,
            hwdec: "no".to_string(),
            maintain_associations: true,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
                log_warn!("配置文件解析失败，使用默认值: {error}");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let Some(path) = config_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = self.write(&path) {
            log_warn!("保存配置失败: {error:#}");
        }
    }

    fn write(&self, path: &std::path::Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        // 先写临时文件再改名：中途崩溃不会留下一个半截的配置文件。
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, text)?;
        std::fs::rename(&temp, path)?;
        Ok(())
    }
}

/// 设置文件位置，遵循各平台惯例。
pub fn config_path() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("NGY_VIDEO_CONFIG_DIR") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir).join("settings.json"));
        }
    }

    #[cfg(windows)]
    {
        let base = std::env::var("APPDATA").ok().map(PathBuf::from)?;
        Some(base.join("ngy-video-player").join("settings.json"))
    }

    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").ok()?;
        Some(
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("ngy-video-player")
                .join("settings.json"),
        )
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let base = std::env::var("XDG_CONFIG_HOME")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|home| PathBuf::from(home).join(".config"))
            })?;
        Some(base.join("ngy-video-player").join("settings.json"))
    }
}

impl Serialize for LoopMode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(match self {
            LoopMode::Off => "off",
            LoopMode::All => "all",
            LoopMode::One => "one",
        })
    }
}

impl<'de> Deserialize<'de> for LoopMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "all" => LoopMode::All,
            "one" => LoopMode::One,
            _ => LoopMode::Off,
        })
    }
}
