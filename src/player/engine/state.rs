//! 对外状态类型：轨道、章节、循环模式、快照与命令。
//!
//! 界面只读这些结构，不碰 mpv 句柄；命令则反向从这里流回引擎线程。

use super::*;

// ---------------------------------------------------------------------------
// 对外类型
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TrackKind {
    #[default]
    Video,
    Audio,
    Subtitle,
}

impl TrackKind {
    pub fn label(self) -> &'static str {
        match self {
            TrackKind::Video => "视频",
            TrackKind::Audio => "音频",
            TrackKind::Subtitle => "字幕",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Track {
    pub id: i64,
    pub kind: TrackKind,
    pub title: String,
    pub lang: String,
    pub codec: String,
    pub selected: bool,
    pub external: bool,
    pub default: bool,
}

impl Track {
    pub fn display_name(&self) -> String {
        if !self.title.is_empty() {
            return self.title.clone();
        }
        let mut parts = Vec::new();
        if !self.lang.is_empty() {
            parts.push(self.lang.to_uppercase());
        }
        if !self.codec.is_empty() {
            parts.push(self.codec.to_uppercase());
        }
        if parts.is_empty() {
            match self.kind {
                TrackKind::Video => format!("#{} 视频", self.id),
                TrackKind::Audio => format!("#{} 音频", self.id),
                TrackKind::Subtitle => format!("#{} 字幕", self.id),
            }
        } else {
            parts.join(" · ")
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Chapter {
    pub title: String,
    pub time: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LoopMode {
    #[default]
    Off,
    One,
    All,
}

impl LoopMode {
    pub fn label(self) -> &'static str {
        match self {
            LoopMode::Off => "顺序播放",
            LoopMode::All => "列表循环",
            LoopMode::One => "单集循环",
        }
    }

    /// 控制条上的两字短标签，用来让当前模式一眼可见。
    pub fn short_label(self) -> &'static str {
        match self {
            LoopMode::Off => "顺序",
            LoopMode::All => "列表",
            LoopMode::One => "单集",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    Eof,
    Stopped,
    Quit,
    Error,
    Redirect,
}

/// 界面需要的一切播放状态。整体用一把锁保护，读取时克隆一份，
/// 避免界面渲染过程中逐字段加锁。
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub paused: bool,
    pub position: f64,
    pub duration: f64,
    pub volume: f64,
    pub muted: bool,
    pub speed: f64,
    pub idle: bool,
    pub seeking: bool,
    pub buffering: bool,
    pub video_width: u32,
    pub video_height: u32,
    pub media_title: String,
    pub path: Option<PathBuf>,
    pub tracks: Vec<Track>,
    pub chapters: Vec<Chapter>,
    pub audio_device: String,
    pub error: Option<String>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            paused: true,
            position: 0.0,
            duration: 0.0,
            volume: 80.0,
            muted: false,
            speed: 1.0,
            idle: true,
            seeking: false,
            buffering: false,
            video_width: 0,
            video_height: 0,
            media_title: String::new(),
            path: None,
            tracks: Vec::new(),
            chapters: Vec::new(),
            audio_device: String::new(),
            error: None,
        }
    }
}

impl Snapshot {
    pub fn has_video(&self) -> bool {
        self.video_width > 0 && self.video_height > 0
    }

    pub fn aspect(&self) -> f32 {
        if self.has_video() {
            self.video_width as f32 / self.video_height as f32
        } else {
            16.0 / 9.0
        }
    }

    pub fn track(&self, kind: TrackKind) -> Vec<&Track> {
        self.tracks.iter().filter(|t| t.kind == kind).collect()
    }

    pub fn selected_track(&self, kind: TrackKind) -> Option<&Track> {
        self.tracks.iter().find(|t| t.kind == kind && t.selected)
    }
}

/// 后台线程推给界面的事件。界面侧用 `cx.spawn` 收，收到就 `notify`。
#[derive(Clone)]
pub enum PlayerEvent {
    /// 状态有变化（进度、暂停、轨道……），重新读一次快照即可。
    Dirty,
    /// 新的一帧画面。
    Frame(Arc<RenderImage>),
    /// 进度条预览图。`time` 是当初请求的时间点（不是实际画面时间，
    /// 因为实际帧会落在关键帧附近）。
    Thumbnail { time: f64, image: Arc<RenderImage> },
    /// 当前文件播放结束。
    Ended(EndReason),
    /// 出错了，界面应该显示出来。
    Error(String),
    /// 截图结果：成功是落盘路径，失败是给用户看的原因。
    Screenshot(std::result::Result<PathBuf, String>),
}

#[derive(Clone, Debug)]
pub enum Command {
    Load {
        url: String,
        start: Option<f64>,
    },
    Stop,
    PlayPause,
    SetPaused(bool),
    SeekAbsolute(f64),
    SeekRelative(f64),
    /// 逐帧步进：正数向前、负数向后（每次只走一帧）。
    StepFrame(i32),
    SetVolume(f64),
    SetMuted(bool),
    SetSpeed(f64),
    SelectTrack {
        kind: TrackKind,
        id: i64,
    },
    AddSubtitle(String),
    Cycle(&'static str),
    SetOption {
        name: String,
        value: String,
    },
    Screenshot(String),
}

// ---------------------------------------------------------------------------
// 引擎
// ---------------------------------------------------------------------------

/// 起引擎时要写进 mpv 选项的偏好。
///
/// 为什么走「initialize 之前的选项」而不是运行期的 `set`：`hwdec` 和
/// `d3d11-adapter` 都是 VO / 解码器建立时才读的选项，程序刚起来那一刻还没有 VO，
/// 用 `set` 可能落空。用户在面板里改的那一次才走 `set`（那时通常已经在放片子）。
pub struct EngineOptions {
    /// `hwdec` 的取值，见 `settings::DecodeMode::hwdec`。
    pub hwdec: String,
    /// D3D11 用哪块显卡（只有 Windows 的 D3D11 后端认这个选项）。
    pub d3d11_adapter: Option<String>,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            // 软件渲染下硬解要把帧从显存倒回内存，反而更慢，默认关闭。
            hwdec: "no".to_string(),
            d3d11_adapter: None,
        }
    }
}
