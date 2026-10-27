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

/// 画面比例。前四档是「按这个比例显示」，后两档是铺满窗口的两种办法。
///
/// 注意「铺满」和「比例」是互斥的：选了 4:3 就不可能同时铺满 16:9 的窗口，
/// 所以用一个枚举而不是「比例 + 铺满开关」两个字段，界面里也就能排成一组单选。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VideoAspect {
    /// 跟随片源自身的显示比例。
    #[default]
    Source,
    R4x3,
    R16x9,
    R21x9,
    /// 铺满屏幕（拉伸）：不保持比例，直接拉满窗口，会变形。
    Stretch,
    /// 铺满屏幕（裁剪）：保持比例放大到铺满，超出的部分裁掉。
    Crop,
}

impl VideoAspect {
    /// 界面里的全部选项，顺序就是展示顺序。
    pub const ALL: [VideoAspect; 6] = [
        VideoAspect::Source,
        VideoAspect::R4x3,
        VideoAspect::R16x9,
        VideoAspect::R21x9,
        VideoAspect::Stretch,
        VideoAspect::Crop,
    ];

    pub fn label(self) -> &'static str {
        match self {
            VideoAspect::Source => "原始比例",
            VideoAspect::R4x3 => "4:3",
            VideoAspect::R16x9 => "16:9",
            VideoAspect::R21x9 => "21:9",
            VideoAspect::Stretch => "铺满屏幕（拉伸）",
            VideoAspect::Crop => "铺满屏幕（裁剪）",
        }
    }

    /// 交给 mpv 的 `video-aspect-override` 取值；`None` 表示跟随片源。
    ///
    /// 铺满那两档也不覆盖比例：拉伸由 `keepaspect=no` 完成，裁剪由 `panscan` 完成。
    pub fn override_value(self) -> Option<&'static str> {
        match self {
            VideoAspect::R4x3 => Some("4:3"),
            VideoAspect::R16x9 => Some("16:9"),
            VideoAspect::R21x9 => Some("21:9"),
            VideoAspect::Source | VideoAspect::Stretch | VideoAspect::Crop => None,
        }
    }

    /// 画面在窗口里固定占用的宽高比；`None` 表示由视频区域本身决定（铺满）。
    pub fn ratio(self) -> Option<f32> {
        match self {
            VideoAspect::R4x3 => Some(4.0 / 3.0),
            VideoAspect::R16x9 => Some(16.0 / 9.0),
            VideoAspect::R21x9 => Some(21.0 / 9.0),
            VideoAspect::Source | VideoAspect::Stretch | VideoAspect::Crop => None,
        }
    }

    /// 是否需要 mpv 关掉等比缩放（拉伸）。
    pub fn stretches(self) -> bool {
        self == VideoAspect::Stretch
    }

    /// 是否需要 mpv 放大并裁掉溢出部分。
    pub fn crops(self) -> bool {
        self == VideoAspect::Crop
    }

    fn tag(self) -> &'static str {
        match self {
            VideoAspect::Source => "source",
            VideoAspect::R4x3 => "4:3",
            VideoAspect::R16x9 => "16:9",
            VideoAspect::R21x9 => "21:9",
            VideoAspect::Stretch => "stretch",
            VideoAspect::Crop => "crop",
        }
    }
}

/// 画面旋转。只留 90 度的整数档：mpv 的旋转滤镜按 90 度工作，任意角度的需求
/// 在播放器里几乎不存在，多一档「自定义角度」只是给自己找边界情况。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rotation {
    #[default]
    None,
    Cw90,
    Cw180,
    Cw270,
}

impl Rotation {
    pub const ALL: [Rotation; 4] = [
        Rotation::None,
        Rotation::Cw90,
        Rotation::Cw180,
        Rotation::Cw270,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Rotation::None => "不旋转",
            Rotation::Cw90 => "90°",
            Rotation::Cw180 => "180°",
            Rotation::Cw270 => "270°",
        }
    }

    /// 顺时针角度，写进配置，也决定拷帧时怎么转。
    pub fn degrees(self) -> u16 {
        match self {
            Rotation::None => 0,
            Rotation::Cw90 => 90,
            Rotation::Cw180 => 180,
            Rotation::Cw270 => 270,
        }
    }

    /// 90/270 度时画面的长宽互换，排版和「不超过原始分辨率」的夹取都要按换过的算。
    pub fn swaps_axes(self) -> bool {
        matches!(self, Rotation::Cw90 | Rotation::Cw270)
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
    /// 画面比例。和「窗口大小」无关，是用户对画质的偏好，所以换片、重启都保留。
    pub video_aspect: VideoAspect,
    /// 画面旋转角度。
    pub video_rotate: Rotation,
    /// 水平镜像（左右翻转）。
    pub mirror_h: bool,
    /// 垂直镜像（上下翻转）。
    pub mirror_v: bool,
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
            video_aspect: VideoAspect::Source,
            video_rotate: Rotation::None,
            mirror_h: false,
            mirror_v: false,
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

/// 比例按 `"4:3"` 这样的字面量存：配置文件是给人看的，改成数字下标
/// 以后调整选项顺序就会悄悄改变用户的选择。
impl Serialize for VideoAspect {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.tag())
    }
}

impl<'de> Deserialize<'de> for VideoAspect {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Ok(VideoAspect::ALL
            .into_iter()
            .find(|mode| mode.tag() == text)
            .unwrap_or_default())
    }
}

/// 旋转存成角度而不是枚举名：手改配置的人一眼能看懂 `"video_rotate": 90`。
impl Serialize for Rotation {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u16(self.degrees())
    }
}

impl<'de> Deserialize<'de> for Rotation {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // 认不出来的角度（手误、别的版本写进来的）一律当作不旋转，
        // 而不是让整份配置解析失败。
        let degrees = u16::deserialize(deserializer)?;
        Ok(Rotation::ALL
            .into_iter()
            .find(|rotation| rotation.degrees() == degrees)
            .unwrap_or_default())
    }
}
