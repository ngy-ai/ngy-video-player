//! 偏好设置：音量、循环模式、面板开合，以及上次打开的目录。
//!
//! 只存「用户改过就希望记住」的东西。播放位置之类不做记忆——大多数场景下
//! 用户并不希望下次打开时从上次中断的地方接着放。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::player::LoopMode;

/// 窗口置顶策略。三档而不是一个开关：看片时想让它压住别的窗口，
/// 暂停去干别的活时又不想它挡路，这两种诉求得分开表达。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OnTop {
    /// 不置顶。
    #[default]
    Never,
    /// 只在真正播放时置顶；暂停、空载、放完都让出层级。
    Playing,
    /// 不管播没播都置顶。
    Always,
}

impl OnTop {
    /// 菜单里的全部选项，顺序就是展示顺序。
    pub const ALL: [OnTop; 3] = [OnTop::Never, OnTop::Playing, OnTop::Always];

    pub fn label(self) -> &'static str {
        match self {
            OnTop::Never => "不置顶",
            OnTop::Playing => "播放时置顶",
            OnTop::Always => "总是置顶",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub volume: f64,
    pub muted: bool,
    pub speed: f64,
    pub loop_mode: LoopMode,
    pub show_playlist: bool,
    /// 置顶策略。早期版本这里叫 `always_on_top` 且是 bool，改名换类型后
    /// 仍认旧键，否则整份配置会解析失败、被当成默认值覆盖掉。
    #[serde(default, alias = "always_on_top")]
    pub on_top: OnTop,
    pub last_dir: Option<PathBuf>,
    pub window_width: f32,
    pub window_height: f32,
    /// 播放列表面板的宽度。用户拖过一次之后按这个值还原。
    ///
    /// 字段级默认值：旧配置文件里没有这一项时按 300 起算，而不是 f32 的 0
    /// （0 虽然会被界面夹到最小宽度，但那是应急，不该当成默认布局）。
    #[serde(default = "default_playlist_width")]
    pub playlist_width: f32,
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
            on_top: OnTop::Never,
            last_dir: None,
            window_width: 1120.0,
            window_height: 680.0,
            playlist_width: 300.0,
            fullscreen: false,
            hwdec: "no".to_string(),
            maintain_associations: true,
        }
    }
}

fn default_playlist_width() -> f32 {
    300.0
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

/// 播放列表存档的位置：和设置文件放同一个目录，但分开存。
///
/// 列表可能一天改十几次，设置基本不动；分开写能避免每次切歌都重写一遍偏好，
/// 出问题时也更容易判断是哪一份文件坏了。
pub fn playlists_path() -> Option<PathBuf> {
    config_path().map(|path| path.with_file_name("playlists.json"))
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

impl Serialize for OnTop {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.tag())
    }
}

impl<'de> Deserialize<'de> for OnTop {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// 旧配置里同一个键是 bool（`always_on_top`）；认不出来时才回落到「不置顶」。
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Bool(bool),
            Text(String),
        }

        Ok(match Raw::deserialize(deserializer)? {
            Raw::Bool(true) => OnTop::Always,
            Raw::Bool(false) => OnTop::Never,
            Raw::Text(text) => match text.as_str() {
                "playing" => OnTop::Playing,
                "always" => OnTop::Always,
                _ => OnTop::Never,
            },
        })
    }
}

impl OnTop {
    fn tag(self) -> &'static str {
        match self {
            OnTop::Never => "never",
            OnTop::Playing => "playing",
            OnTop::Always => "always",
        }
    }
}
