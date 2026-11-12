//! 界面自己的小状态类型：横幅、右键菜单、页签重命名、悬停预览、截图卡片、
//! A-B 循环标记、菜单栏的菜单 / 条目，以及小窗观察用的 [`MiniState`]。
//!
//! 它们都只服务界面，不参与工程存盘；从 `app.rs` 挪出来是因为那个文件被
//! `PlayerApp` 的字段表撑到了 870 行。

use super::*;

#[derive(Clone, Debug, Default)]
pub struct LaunchOptions {
    pub paths: Vec<PathBuf>,
    pub fullscreen: bool,
}

/// 底部红色横幅。播放出错和文件关联失败共用同一块位置，只是标题不同。
#[derive(Clone, Debug)]
pub(in crate::app) struct Banner {
    pub(in crate::app) title: &'static str,
    pub(in crate::app) body: String,
}

impl Banner {
    pub(in crate::app) fn new(title: &'static str, body: impl Into<String>) -> Self {
        Self {
            title,
            body: body.into(),
        }
    }
}

/// 播放列表行的右键菜单。位置是窗口坐标，弹窗按它绝对定位。
pub(in crate::app) struct ItemMenu {
    pub(in crate::app) index: usize,
    pub(in crate::app) path: PathBuf,
    /// 这一行是文件夹。菜单项不同：文件夹没有「查看格式」，多一项展开/收起。
    pub(in crate::app) dir: bool,
    pub(in crate::app) position: Point<Pixels>,
}

/// 播放列表页签的右键菜单。位置是窗口坐标，弹窗按它绝对定位。
pub(in crate::app) struct TabMenu {
    pub(in crate::app) index: usize,
    pub(in crate::app) position: Point<Pixels>,
}

/// 正在重命名的列表页：改的是哪一页、已经输入了什么。
pub(in crate::app) struct TabRename {
    pub(in crate::app) index: usize,
    pub(in crate::app) text: String,
    /// 用户已经动过键盘。预填的名字只是让人知道正在改哪一页，第一次敲字
    /// 就把它整个换掉——列表名基本是整条重写，不是接着往后补。
    pub(in crate::app) typed: bool,
}

/// 「查看格式」面板。探测要读文件，所以先开一个 Loading 状态再填内容。
pub(in crate::app) struct FileInfoPanel {
    pub(in crate::app) path: PathBuf,
    pub(in crate::app) state: InfoState,
}

pub(in crate::app) enum InfoState {
    Loading,
    Ready(crate::player::MediaInfo),
    Failed(String),
}

/// 进度条悬停预览的即时状态。
pub(in crate::app) struct HoverPreview {
    /// 鼠标指向的时间点（秒）。
    pub(in crate::app) time: f64,
    /// 在进度条上的横向比例（0..1），决定浮层停在哪。
    pub(in crate::app) ratio: f32,
    /// 已经拿到的预览图。
    ///
    /// 换位置后先沿用上一张，等新的回来再替换：每动一下鼠标就把浮层清空，
    /// 看起来比「慢半拍」更难受，而且会一直闪。
    pub(in crate::app) image: Option<Arc<RenderImage>>,
}

/// A-B 循环的两个标记点。
///
/// `path` 记下标记时的文件：换片后位置与时长都变了，旧标记必须失效，
/// 不能在别的片子上继续循环。
#[derive(Clone, Debug, Default)]
pub(in crate::app) struct AbLoop {
    pub(in crate::app) path: Option<PathBuf>,
    pub(in crate::app) start: Option<f64>,
    pub(in crate::app) end: Option<f64>,
    /// 上一次回跳的时刻，用来避开 seek 的延迟。
    pub(in crate::app) seeked_at: Option<Instant>,
}

/// 截图卡片里那块预览的三种状态。
///
/// 用枚举而不是 `Option`：`None` 分不清「还在解码」和「解码失败」，
/// 界面只能一直显示「正在读取」，看起来像卡住了。
pub(in crate::app) enum ScreenshotThumb {
    Loading,
    Ready(Arc<RenderImage>),
    Failed,
}

/// 截图成功后的操作卡片：预览 + 复制 / 打开 / 定位。
pub(in crate::app) struct ScreenshotCard {
    pub(in crate::app) path: PathBuf,
    pub(in crate::app) thumb: ScreenshotThumb,
    pub(in crate::app) shown_at: Instant,
    /// 鼠标停在卡片上。这时候不自动收起——正要点「复制」时卡片消失最气人。
    pub(in crate::app) hovered: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Menu {
    Subtitle,
    Audio,
    Speed,
    /// 倍速菜单里的自定义滑块，是 `Speed` 的第二层。
    SpeedCustom,
    Loop,
}

impl Menu {
    /// 倍速按钮按下时，预设列表和自定义面板算同一个浮层：再点一次一起收起。
    pub(in crate::app) fn is_speed(self) -> bool {
        matches!(self, Menu::Speed | Menu::SpeedCustom)
    }
}

/// 悬浮菜单里的一项点击后要做什么。
#[derive(Clone)]
pub(in crate::app) enum MenuAction {
    Command(Command),
    Loop(LoopMode),
    /// 预设倍速。不能直接发给 mpv：它同时是「记住的偏好」，得走 `set_speed`。
    Speed(f64),
    /// 从预设列表进入自定义倍速面板。
    SpeedCustom,
    /// 字幕延迟（秒）：正数延后、负数提前、0 归零。
    ///
    /// 不能直接塞 `Command::SetOption` 发出去：界面要记一份当前值才能显示，
    /// 而且换片时得跟着清掉。
    SubtitleDelay(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum BarMenu {
    File,
    /// 两种模式都有，内容各自一套：播放模式放单文件处理工具，剪辑模式放时间线编辑命令。
    Edit,
    Playback,
    /// 剪辑模式的工程预览控制。
    Preview,
    Subtitle,
    Audio,
    View,
    Settings,
    Help,
}

impl BarMenu {
    /// 播放模式：文件 + 编辑（单文件工具）+ 播放/字幕/音轨 + 视图/设置/帮助。
    pub(in crate::app) const PLAYBACK: &'static [BarMenu] = &[
        BarMenu::File,
        BarMenu::Edit,
        BarMenu::Playback,
        BarMenu::Subtitle,
        BarMenu::Audio,
        BarMenu::View,
        BarMenu::Settings,
        BarMenu::Help,
    ];

    /// 剪辑模式：字幕 / 音轨切换针对的是播放列表里的片子，工程预览没有这套概念；
    /// 换成「编辑 / 预览」两组剪辑动作。两套菜单只共享文件、编辑、视图、设置、帮助五个
    /// 标题，其中「编辑」在播放模式放单文件工具、在剪辑模式放时间线命令。
    pub(in crate::app) const EDITING: &'static [BarMenu] = &[
        BarMenu::File,
        BarMenu::Edit,
        BarMenu::Preview,
        BarMenu::View,
        BarMenu::Settings,
        BarMenu::Help,
    ];

    pub(in crate::app) fn for_mode(editing: bool) -> &'static [BarMenu] {
        if editing {
            Self::EDITING
        } else {
            Self::PLAYBACK
        }
    }

    pub(in crate::app) fn label(self) -> &'static str {
        match self {
            BarMenu::File => "文件",
            BarMenu::Edit => "编辑",
            BarMenu::Playback => "播放",
            BarMenu::Preview => "预览",
            BarMenu::Subtitle => "字幕",
            BarMenu::Audio => "音轨",
            BarMenu::View => "视图",
            BarMenu::Settings => "设置",
            BarMenu::Help => "帮助",
        }
    }
}

pub(in crate::app) enum BarItem {
    Separator,
    Entry(BarEntry),
}

pub(in crate::app) struct BarEntry {
    pub(in crate::app) label: String,
    pub(in crate::app) hint: Option<&'static str>,
    pub(in crate::app) active: bool,
    pub(in crate::app) enabled: bool,
    pub(in crate::app) action: Option<BarAction>,
}

impl BarEntry {
    pub(in crate::app) fn new(label: impl Into<String>, action: BarAction) -> Self {
        Self {
            label: label.into(),
            hint: None,
            active: false,
            enabled: true,
            action: Some(action),
        }
    }

    pub(in crate::app) fn note(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            hint: None,
            active: false,
            enabled: false,
            action: None,
        }
    }

    pub(in crate::app) fn hint(mut self, hint: &'static str) -> Self {
        self.hint = Some(hint);
        self
    }

    pub(in crate::app) fn checked(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub(in crate::app) fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// 轨道编辑对话框处理哪一路。音轨与字幕分开成两个入口、两个对话框，
/// 共用同一套状态机与后台任务，只是标题、按钮和说明各按自己那一路来。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum EditTarget {
    Audio,
    Subtitle,
}

impl EditTarget {
    pub(in crate::app) fn title(self) -> &'static str {
        match self {
            EditTarget::Audio => "编辑音轨",
            EditTarget::Subtitle => "内嵌字幕",
        }
    }

    /// 对话框里那一行的小标题。
    pub(in crate::app) fn label(self) -> &'static str {
        match self {
            EditTarget::Audio => "音轨",
            EditTarget::Subtitle => "字幕",
        }
    }
}

/// 会丢掉当前工程的动作用它排队：有未保存改动时，先确认再执行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) enum AfterSaveConfirm {
    NewProject,
    CloseProject,
    OpenProject(PathBuf),
    Quit,
}

/// 小窗要显示的那点状态。
///
/// **由小窗观察主窗口拉过来，而不是主窗口 push，也不是小窗在 render 里自己去读
/// `PlayerApp`**。两个方向都会踩同一个坑——`PlayerApp` 和 `MiniPlayer` 互相 lease：
///
/// - 小窗 render 里读 `PlayerApp`：小窗的第一次绘制发生在主窗口的点击回调里
///   （`open_window` 会立刻画一帧），那一刻 `PlayerApp` 正被可变借用。
/// - 主窗口 push 给小窗：小窗按钮的回调跑在 `MiniPlayer` 的 update 里，
///   主窗口回手 `mini.update(...)` 就把它 lease 了第二次。
///
/// 观察回调在 effect 刷新阶段才跑，两边都没被借用，所以是唯一安全的方向。
#[derive(Clone, Default)]
pub(in crate::app) struct MiniState {
    pub(in crate::app) title: SharedString,
    pub(in crate::app) position: f64,
    pub(in crate::app) duration: f64,
    pub(in crate::app) playing: bool,
    pub(in crate::app) volume: f64,
    pub(in crate::app) muted: bool,
    /// 播放列表是否为空——空的时候画面区提示「没有正在播放的文件」。
    pub(in crate::app) empty: bool,
    /// 当前播放页的条目数。小窗的上一集 / 下一集按钮据此决定是否可点。
    pub(in crate::app) items: usize,
    /// 循环模式。右键菜单显示当前值，切换后小窗也要跟着更新。
    pub(in crate::app) loop_mode: LoopMode,
    /// 播放速度。`[` / `]` 调完之后小窗要浮一条当前倍速。
    pub(in crate::app) speed: f64,
    /// 当前有没有选中字幕轨。
    pub(in crate::app) subtitle: bool,
    /// 文件里有没有字幕轨；没有时字幕按钮置灰。
    pub(in crate::app) has_subtitle: bool,
    /// 正在播纯音频：没有画面时不显示「正在加载…」占位。
    pub(in crate::app) audio_only: bool,
}

impl MiniState {
    /// 和另一份状态是否「看起来一样」。进度和音量按百分之一内视为没变，
    /// 免得每帧都判定成变化而白重绘一次小窗。
    pub(in crate::app) fn same_as(&self, other: &Self) -> bool {
        self.title == other.title
            && self.empty == other.empty
            && self.items == other.items
            && self.loop_mode == other.loop_mode
            && (self.speed - other.speed).abs() < 0.001
            && self.subtitle == other.subtitle
            && self.has_subtitle == other.has_subtitle
            && self.audio_only == other.audio_only
            && self.playing == other.playing
            && self.muted == other.muted
            && (self.position - other.position).abs() < 0.005
            && (self.duration - other.duration).abs() < 0.005
            && (self.volume - other.volume).abs() < 0.005
    }
}
