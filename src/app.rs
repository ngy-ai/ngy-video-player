//! 主视图：窗口、输入、布局。
//!
//! 一条原则贯穿整个模块：**界面是唯一的状态源**。mpv 只负责解码和输出帧，
//! 播放列表、循环模式、面板开合、拖动位置全部由这里持有。mpv 的属性变化只是
//! 「事实汇报」（现在第几秒、有没有暂停），收到后更新快照、触发重绘。
//!
//! 本文件只放入口、共享状态类型和 `PlayerApp` 的字段：字段保持私有，
//! 子模块作为后代模块才能自由读写。具体实现按职责拆在 `app/` 下的子模块，
//! `PlayerApp` 的多个 `impl` 块分散在各文件里，子模块统一 `use super::*;`
//! 拿到父模块的导入、类型与 `actions!` 生成的动作。

use crate::ui::IconName;
use crate::{
    formats, platform,
    player::{
        Command, EndReason, Engine, EngineOptions, LoopMode, PlayerEvent, Snapshot, TrackKind,
    },
    playlist::{self, Playlist, PlaylistSet},
    settings::{DecodeMode, OnTop, RenderMode, Rotation, Settings, VideoAspect},
    ui::{
        self, bg, format_time, icon, icon_button, icon_button_with_tip, pill_button_with_tip,
        surface, text, text_faint, text_muted,
    },
};
use anyhow::Result;
use futures::{StreamExt as _, channel::mpsc::UnboundedReceiver};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext, Bounds, ClickEvent, Context, Corners,
    CursorStyle, ElementId, Entity, ExternalPaths, FocusHandle, Hsla, IntoElement, KeyBinding,
    KeyDownEvent, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, Render, RenderImage, ScrollDelta, ScrollWheelEvent, SharedString, Styled, WeakEntity,
    Window, WindowBounds, WindowHandle, WindowKind, WindowOptions, WindowVisibility, actions,
    canvas, div, img, point, px, size,
};
use std::{
    cell::Cell,
    collections::{BTreeSet, VecDeque},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

actions!(
    player,
    [
        TogglePlay,
        SeekForward,
        SeekBackward,
        VolumeUp,
        VolumeDown,
        ToggleMute,
        ToggleFullscreen,
        ToggleMini,
        TogglePlaylist,
        OpenFile,
        NextFile,
        PreviousFile,
        TakeScreenshot,
        SpeedUp,
        SpeedDown,
        StopPlayback,
        ToggleHelp,
        OpenPictureSettings,
        ExitOverlay,
        DeleteSelected,
    ]
);

// 子模块按职责拆分：`PlayerApp` 的方法分散在各文件里，Rust 允许同一类型有多个 impl 块。
mod associations;
mod bar;
mod constants;
mod controls;
mod format;
mod menu_bar;
mod menus;
mod mini;
mod overlays;
mod picture;
mod playback;
mod playlist_ops;
mod playlist_ui;
mod render;
mod session;
mod settings_panel;
mod video_area;
mod widgets;
mod window;

use self::constants::*;
use self::format::*;
use self::mini::MiniPlayer;
use self::settings_panel::SettingsTab;
use self::widgets::*;

#[derive(Clone, Debug, Default)]
pub struct LaunchOptions {
    pub paths: Vec<PathBuf>,
    pub fullscreen: bool,
}

/// 底部红色横幅。播放出错和文件关联失败共用同一块位置，只是标题不同。
#[derive(Clone, Debug)]
struct Banner {
    title: &'static str,
    body: String,
}

impl Banner {
    fn new(title: &'static str, body: impl Into<String>) -> Self {
        Self {
            title,
            body: body.into(),
        }
    }
}

/// 播放列表行的右键菜单。位置是窗口坐标，弹窗按它绝对定位。
struct ItemMenu {
    index: usize,
    path: PathBuf,
    position: Point<Pixels>,
}

/// 「查看格式」面板。探测要读文件，所以先开一个 Loading 状态再填内容。
struct FileInfoPanel {
    path: PathBuf,
    state: InfoState,
}

enum InfoState {
    Loading,
    Ready(crate::player::MediaInfo),
    Failed(String),
}

/// 进度条悬停预览的即时状态。
struct HoverPreview {
    /// 鼠标指向的时间点（秒）。
    time: f64,
    /// 在进度条上的横向比例（0..1），决定浮层停在哪。
    ratio: f32,
    /// 已经拿到的预览图。
    ///
    /// 换位置后先沿用上一张，等新的回来再替换：每动一下鼠标就把浮层清空，
    /// 看起来比「慢半拍」更难受，而且会一直闪。
    image: Option<Arc<RenderImage>>,
}

/// 截图卡片里那块预览的三种状态。
///
/// 用枚举而不是 `Option`：`None` 分不清「还在解码」和「解码失败」，
/// 界面只能一直显示「正在读取」，看起来像卡住了。
enum ScreenshotThumb {
    Loading,
    Ready(Arc<RenderImage>),
    Failed,
}

/// 截图成功后的操作卡片：预览 + 复制 / 打开 / 定位。
struct ScreenshotCard {
    path: PathBuf,
    thumb: ScreenshotThumb,
    shown_at: Instant,
    /// 鼠标停在卡片上。这时候不自动收起——正要点「复制」时卡片消失最气人。
    hovered: bool,
}

pub struct PlayerApp {
    engine: Option<Arc<Engine>>,
    snapshot: Snapshot,
    /// 正在显示的那一帧。
    frame: Option<Arc<RenderImage>>,
    /// 刚从后台收到、还没换上的帧。
    incoming: Option<Arc<RenderImage>>,
    /// 已经画过、等待归还给 GPU atlas 的旧帧。
    ///
    /// 保留两帧再释放：`drop_image` 会立刻回收 atlas 里的区域，而上上帧可能还在
    /// GPU 队列里被采样，留一点余量可以彻底避免「偶尔闪一下」。
    retired: VecDeque<Arc<RenderImage>>,

    /// 面板上的全部列表。播放、删除、清空都只作用于当前页。
    playlists: PlaylistSet,
    /// 正在播的那一页。切页不打断播放，所以它未必等于当前显示页。
    playing_tab: Option<usize>,
    /// 列表里被选中的行。索引只对当前这一页有效，任何增删、切页都要清空。
    selection: BTreeSet<usize>,
    /// Shift 连选的起点（最后一次普通点击或叠加点击的位置）。
    selection_anchor: Option<usize>,
    /// 正在拖播放列表的左边界。
    playlist_dragging: bool,
    /// 播放列表行的右键菜单。
    item_menu: Option<ItemMenu>,
    /// 「查看格式」面板；`None` 表示没打开。
    file_info: Option<FileInfoPanel>,
    settings: Settings,
    /// 上一次真正下发给窗口系统的置顶状态。
    ///
    /// 置顶是窗口系统的事，不是界面每帧都要重设的属性；比对一下，只在
    /// 「想置顶」和「已经在置顶」不一致时才调平台 API。
    on_top_applied: bool,
    focus: FocusHandle,

    /// 鼠标最后一次活动的时间，用来决定控制条什么时候淡出。
    last_activity: Instant,
    controls_forced: bool,
    /// 进度条拖动中的临时位置（秒）；`None` 表示没在拖。
    dragging: Option<f64>,
    /// 进度条自身的 bounds，由 canvas 在 prepaint 阶段写回。
    progress_bounds: Rc<Cell<Bounds<Pixels>>>,
    volume_bounds: Rc<Cell<Bounds<Pixels>>>,
    volume_dragging: bool,
    /// 自定义倍速滑块的 bounds，由 canvas 在 prepaint 阶段写回。
    speed_bounds: Rc<Cell<Bounds<Pixels>>>,
    speed_dragging: bool,
    /// Ctrl/⌘ + 滚轮的像素余量：触控板一次滑动拆成很多小增量，攒够一格才走一帧。
    frame_scroll: f32,
    /// 进度条悬停预览；`None` 表示鼠标不在进度条上。
    hover: Option<HoverPreview>,
    /// 已经发出、还没回来的预览请求（时间点 + 发出时刻）。
    ///
    /// 同一时刻只挂一条：悬停时指针每动几个像素就产生一次请求，全都发出去只会
    /// 让预览永远追着指针跑。结果回来后再按「当时」的位置补一条，最终收敛。
    preview_request: Option<(f64, Instant)>,
    /// 已经画过、等待归还给 GPU atlas 的旧预览图。规则与 `retired` 相同。
    preview_retired: VecDeque<Arc<RenderImage>>,

    /// 原生文件选择框是否已经打开。
    ///
    /// rfd 的对话框阻塞在后台线程里，主窗口在拿到结果前不会知道自己「正在等对话框」。
    /// 不挡一下的话，连按 O 会叠出多个对话框，所以这里自己记一笔。
    dialog_open: bool,

    osd: Option<(Vec<String>, Instant)>,
    /// 最近一张截图的操作卡片；`None` 表示没显示。
    screenshot_card: Option<ScreenshotCard>,
    error: Option<Banner>,
    /// 设置窗口是否打开。所有偏好都收在那一个窗口里，这里只记开合。
    show_settings: bool,
    /// 设置窗口当前停在哪个标签页。收起再打开时回到上次看的那一页。
    settings_tab: SettingsTab,
    /// 悬浮菜单（字幕 / 音轨 / 循环）当前展开的是哪一个。
    menu: Option<Menu>,
    /// 顶部菜单栏当前展开的是哪一个。
    menu_bar: Option<BarMenu>,
    /// 小窗（画中画）的视图；`None` 表示当前没开小窗。
    ///
    /// 小窗与主窗口共用同一路解码：它只负责把主窗口转发过来的帧画出来，
    /// 播放状态、播放列表、mpv 句柄都仍然只有主窗口这一份。
    mini: Option<Entity<MiniPlayer>>,
    /// 小窗的窗口句柄：退出小窗时要靠它把窗口关掉，而不是只清掉引用。
    mini_window: Option<WindowHandle<MiniPlayer>>,
    /// 主窗口正被**有意**还原（小窗上的还原按钮 / Esc / T）。
    ///
    /// 小窗模式下主窗口的每一次「变可见」都会被可见性观察者当成意外重新收起
    /// （Windows 上主窗口是藏起来的，正常情况下没有这个入口；藏不起来的那两个
    /// 平台上就是「用户点了任务栏」）。这条路径要把自己触发的那一次摘出去，
    /// 否则刚还原的主窗口会立刻又被收回去——两个窗口都不可见，程序看起来直接消失。
    main_restoring: bool,
    /// 主窗口句柄。小窗模式下主窗口不可见（Windows 上是藏着，其它平台是最小化），
    /// 还原时要从「小窗那一侧」把它拉回来。
    main_window: AnyWindowHandle,
    /// 文件关联的落点缓存。
    ///
    /// 查询要逐个读注册表，不能放进每帧都跑的渲染路径里；只在展开「设置」菜单、
    /// 或者刚改过关联时刷新一次。
    association_status: platform::associations::Status,
    /// 本机显卡列表。
    ///
    /// 枚举要走系统 API（Windows 上是 `EnumDisplayDevicesW`），不能塞进每帧都跑的
    /// 渲染路径，所以建视图时取一次。中途插拔外置显卡不会自动更新——那种场景
    /// 用户重启一次播放器远比我们做热插拔监听划算。
    gpu_adapters: Vec<String>,

    startup_failure: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Menu {
    Subtitle,
    Audio,
    Speed,
    /// 倍速菜单里的自定义滑块，是 `Speed` 的第二层。
    SpeedCustom,
    Loop,
}

impl Menu {
    /// 倍速按钮按下时，预设列表和自定义面板算同一个浮层：再点一次一起收起。
    fn is_speed(self) -> bool {
        matches!(self, Menu::Speed | Menu::SpeedCustom)
    }
}

/// 悬浮菜单里的一项点击后要做什么。
#[derive(Clone)]
enum MenuAction {
    Command(Command),
    Loop(LoopMode),
    /// 从预设列表进入自定义倍速面板。
    SpeedCustom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BarMenu {
    File,
    Playback,
    Subtitle,
    Audio,
    View,
    Settings,
    Help,
}

impl BarMenu {
    const ALL: [BarMenu; 7] = [
        BarMenu::File,
        BarMenu::Playback,
        BarMenu::Subtitle,
        BarMenu::Audio,
        BarMenu::View,
        BarMenu::Settings,
        BarMenu::Help,
    ];

    fn label(self) -> &'static str {
        match self {
            BarMenu::File => "文件",
            BarMenu::Playback => "播放",
            BarMenu::Subtitle => "字幕",
            BarMenu::Audio => "音轨",
            BarMenu::View => "视图",
            BarMenu::Settings => "设置",
            BarMenu::Help => "帮助",
        }
    }
}

enum BarItem {
    Separator,
    Entry(BarEntry),
}

struct BarEntry {
    label: String,
    hint: Option<&'static str>,
    active: bool,
    enabled: bool,
    action: Option<BarAction>,
}

impl BarEntry {
    fn new(label: impl Into<String>, action: BarAction) -> Self {
        Self {
            label: label.into(),
            hint: None,
            active: false,
            enabled: true,
            action: Some(action),
        }
    }

    fn note(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            hint: None,
            active: false,
            enabled: false,
            action: None,
        }
    }

    fn hint(mut self, hint: &'static str) -> Self {
        self.hint = Some(hint);
        self
    }

    fn checked(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// 菜单动作必须是 `Clone`：点击回调是 `Fn`，可能被多次调用，不能把动作 move 走。
#[derive(Clone)]
enum BarAction {
    Command(Command),
    OpenFile,
    OpenFolder,
    LoadSubtitle,
    Previous,
    Next,
    Seek(f64),
    SetLoop(LoopMode),
    SetOnTop(OnTop),
    TogglePlaylist,
    ToggleFullscreen,
    ToggleMini,
    ToggleHelp,
    /// 打开设置窗口；`Some` 表示直接切到那一页。
    OpenSettings(Option<SettingsTab>),
    TakeScreenshot,
    SetAsDefaultPlayer,
    OpenDefaultApps,
    RemoveAssociations,
    ToggleMaintainAssociations,
    About,
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
struct MiniState {
    title: SharedString,
    position: f64,
    duration: f64,
    playing: bool,
    volume: f64,
    muted: bool,
    /// 播放列表是否为空——空的时候画面区提示「没有正在播放的文件」。
    empty: bool,
}

impl MiniState {
    /// 和另一份状态是否「看起来一样」。进度和音量按百分之一内视为没变，
    /// 免得每帧都判定成变化而白重绘一次小窗。
    fn same_as(&self, other: &Self) -> bool {
        self.title == other.title
            && self.empty == other.empty
            && self.playing == other.playing
            && self.muted == other.muted
            && (self.position - other.position).abs() < 0.005
            && (self.duration - other.duration).abs() < 0.005
            && (self.volume - other.volume).abs() < 0.005
    }
}

pub fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("space", TogglePlay, None),
        KeyBinding::new("right", SeekForward, None),
        KeyBinding::new("left", SeekBackward, None),
        KeyBinding::new("up", VolumeUp, None),
        KeyBinding::new("down", VolumeDown, None),
        KeyBinding::new("m", ToggleMute, None),
        KeyBinding::new("f", ToggleFullscreen, None),
        KeyBinding::new("t", ToggleMini, None),
        KeyBinding::new("a", OpenPictureSettings, None),
        KeyBinding::new("l", TogglePlaylist, None),
        KeyBinding::new("o", OpenFile, None),
        KeyBinding::new("n", NextFile, None),
        KeyBinding::new("p", PreviousFile, None),
        KeyBinding::new("[", SpeedDown, None),
        KeyBinding::new("]", SpeedUp, None),
        KeyBinding::new("s", TakeScreenshot, None),
        KeyBinding::new("escape", ExitOverlay, None),
        KeyBinding::new("delete", DeleteSelected, None),
        // `?` 不在这里绑定：键位串按键帽字符匹配，`?` 得写成 `shift-/`，
        // 且随键盘布局变化。改由视图的 key_down 按实际字符处理。
    ]
}

/// 打开主窗口。`options.paths` 里的文件会被直接加入播放列表并开始播放。
pub fn run(options: LaunchOptions) -> Result<()> {
    let settings = Settings::load();

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx: &mut App| {
            crate::logging::init();
            gpui_kit::init(cx);
            cx.bind_keys(key_bindings());

            let bounds = Bounds::new(
                point(px(120.0), px(90.0)),
                size(px(settings.window_width), px(settings.window_height)),
            );
            let options = options.clone();

            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                move |window, cx| {
                    let view = cx.new(|cx| PlayerApp::new(options.clone(), window, cx));
                    view.update(cx, |_, cx| cx.notify());
                    view
                },
            )
            .expect("无法创建窗口");
        });

    Ok(())
}
