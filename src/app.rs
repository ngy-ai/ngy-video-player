//! 主视图：窗口、输入、布局。
//!
//! 一条原则贯穿全文件：**界面是唯一的状态源**。mpv 只负责解码和输出帧，
//! 播放列表、循环模式、面板开合、拖动位置全部由这里持有。mpv 的属性变化只是
//! 「事实汇报」（现在第几秒、有没有暂停），收到后更新快照、触发重绘。

use crate::ui::IconName;
use crate::{
    formats, platform,
    player::{Command, EndReason, Engine, LoopMode, PlayerEvent, Snapshot, TrackKind},
    playlist::{self, Playlist, PlaylistSet},
    settings::Settings,
    ui::{
        self, bg, format_time, icon, icon_button, pill_button, surface, text, text_faint,
        text_muted,
    },
};
use anyhow::Result;
use futures::{StreamExt as _, channel::mpsc::UnboundedReceiver};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Bounds, ClickEvent, Context, Corners, CursorStyle, ExternalPaths, FocusHandle,
    Hsla, IntoElement, KeyBinding, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, Render, RenderImage, ScrollDelta,
    ScrollWheelEvent, SharedString, Styled, Window, WindowBounds, WindowOptions, actions, canvas,
    div, img, point, px, size,
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
        TogglePlaylist,
        OpenFile,
        NextFile,
        PreviousFile,
        TakeScreenshot,
        SpeedUp,
        SpeedDown,
        StopPlayback,
        ToggleHelp,
        ExitOverlay,
        DeleteSelected,
    ]
);

/// 控制条上的倍速文字：`1` → `1×`，`1.25` → `1.25×`。
fn format_speed(speed: f64) -> String {
    let mut text = format!("{speed:.2}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    format!("{text}×")
}

/// 自定义滑块气泡上的倍速：整数档写成 `1.0×`，更细的档位照实显示 `1.25×`。
fn format_speed_value(speed: f64) -> String {
    let rounded = (speed * 100.0).round() / 100.0;
    if (rounded * 10.0 - (rounded * 10.0).round()).abs() < 0.001 {
        format!("{rounded:.1}×")
    } else {
        format!("{rounded:.2}×")
    }
}

/// 从 mpv 的完整命令错误里取出最后一段（真正的失败原因），放进 OSD 不至于撑满窗口。
fn error_reason(message: &str) -> String {
    let reason = message.rsplit(": ").next().unwrap_or(message).trim();
    let mut chars = reason.chars();
    let short: String = chars.by_ref().take(48).collect();
    if chars.next().is_some() {
        format!("{short}…")
    } else {
        short
    }
}

/// 滑块两端的量程标签。固定宽度，滑轨不会因为数字长短而左右跳。
fn speed_edge_label(label: &'static str, width: f32) -> impl IntoElement {
    div()
        .w(px(width))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(11.0))
        .text_color(text_faint())
        .child(label)
}

/// `8_400_000_000` → `8.4 GB`。文件大小用二进制单位，和资源管理器说的一致。
fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// 右键菜单里的一行：图标 + 文字，整行可点。
fn item_menu_entry(
    id: &'static str,
    icon_name: IconName,
    label: &'static str,
    color: Hsla,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.0))
        .px(px(10.0))
        .py(px(7.0))
        .mx(px(4.0))
        .rounded(px(6.0))
        .text_size(px(13.0))
        .text_color(color)
        .cursor(CursorStyle::PointingHand)
        .hover(|style| style.bg(ui::surface_hover()))
        .child(ui::icon(icon_name, 14.0, color))
        .child(label)
        .on_click(on_click)
        .into_any_element()
}

/// 截图卡片底部的一个动作：图标 + 文字，三等分卡片宽度。
fn screenshot_card_action(
    id: &'static str,
    icon_name: IconName,
    label: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex_1()
        .h(px(36.0))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .cursor(CursorStyle::PointingHand)
        .hover(|style| style.bg(ui::surface_hover()))
        .child(ui::icon(icon_name, 14.0, text_muted()))
        .child(div().text_size(px(12.0)).text_color(text()).child(label))
        .on_click(
            move |event: &ClickEvent, window: &mut Window, cx: &mut App| {
                // 卡片浮在画面上，动作不截断的话同一次点击还会漏到视频区暂停播放。
                cx.stop_propagation();
                on_click(event, window, cx);
            },
        )
}

/// 卡片预览区的占位内容：预览还没解码好、或读不出来时顶上去。
fn placeholder(message: &'static str) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .text_size(px(11.0))
                .text_color(text_faint())
                .child(message),
        )
        .into_any_element()
}

/// 截图卡片里三个动作之间的竖线。
fn card_divider() -> impl IntoElement {
    div().w(px(1.0)).h(px(18.0)).bg(gpui_kit::rgb(0x2c2c34))
}

/// 「查看格式」的正文：容器、时长、大小，再逐条列轨道。
fn render_media_info(info: &crate::player::MediaInfo) -> AnyElement {
    let mut summary: Vec<(String, String)> = Vec::new();
    if !info.container.is_empty() {
        summary.push(("容器".to_string(), info.container.clone()));
    }
    if info.duration > 0.0 {
        summary.push(("时长".to_string(), format_time(info.duration)));
    }
    if let Some(size) = info.size {
        summary.push(("大小".to_string(), format_size(size)));
    }

    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .children(summary.into_iter().map(|(key, value)| {
            div()
                .flex()
                .justify_between()
                .gap(px(16.0))
                .text_size(px(13.0))
                .child(div().text_color(text_muted()).child(key))
                .child(div().text_color(text()).child(value))
        }))
        .child(
            div()
                .mt(px(6.0))
                .text_size(px(12.0))
                .text_color(text_muted())
                .child(format!("轨道 · {}", info.tracks.len())),
        )
        .children(info.tracks.iter().map(|track| {
            let mut meta: Vec<String> = Vec::new();
            if !track.language.is_empty() {
                meta.push(track.language.to_uppercase());
            }
            if !track.codec.is_empty() {
                meta.push(track.codec.clone());
            }
            if !track.title.is_empty() {
                meta.push(track.title.clone());
            }
            if !track.detail.is_empty() {
                meta.push(track.detail.clone());
            }
            if track.external {
                meta.push("外挂".to_string());
            }
            if meta.is_empty() {
                meta.push("—".to_string());
            }

            div()
                .flex()
                .items_start()
                .gap(px(10.0))
                .text_size(px(13.0))
                .child(div().w(px(34.0)).text_color(ui::accent()).child(track.kind))
                .child(div().flex_1().text_color(text()).child(meta.join(" · ")))
        }))
        .into_any_element()
}

/// 给新列表起名字：文件夹名 / m3u 文件名 / 文件所在目录名。
///
/// 名字是列表在面板上唯一的身份，叫「列表 3」的话开三个页就分不清谁是谁了。
fn tab_name_for(paths: &[PathBuf]) -> Option<String> {
    let first = paths.first()?;
    let name = if first.is_dir() {
        first.file_name()
    } else if formats::is_playlist(first) {
        first.file_stem()
    } else {
        first.parent().and_then(|parent| parent.file_name())
    }?;
    let name = name.to_string_lossy().trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// 当前 exe 是不是 `cargo build` 的产物。
///
/// 关联里存的是绝对路径，`target` 目录会被 `cargo clean` 抹掉、被新构建顶替，
/// 所以在用户点下按钮的那一刻值得提一句。
fn is_development_build(exe: &std::path::Path) -> bool {
    let Some(directory) = exe.parent() else {
        return false;
    };
    let under_target = directory
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        == Some("target");
    let profile = directory.file_name().and_then(|name| name.to_str());
    under_target && matches!(profile, Some("debug" | "release"))
}

/// 注册项指向的路径是否覆盖当前可执行文件。
///
/// Windows/Linux 上存的就是 exe 本身；macOS 上存的是 `.app` 目录，exe 在它里面，
/// 所以不能直接比相等。
fn association_target_matches(target: &std::path::Path) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    exe == target || exe.starts_with(target)
}

/// 「设置」菜单里那行只读状态。
fn association_status_line(status: &platform::associations::Status, supported: bool) -> String {
    if !supported {
        return "已注册关联：当前平台尚未支持".to_string();
    }
    if !status.is_registered() {
        return "已注册关联：未注册".to_string();
    }
    let stale = status
        .target
        .as_ref()
        .is_some_and(|target| !association_target_matches(target));
    if stale {
        format!("已注册关联：{} 个格式（路径已过期）", status.registered)
    } else {
        let total = platform::associations::count();
        format!("已注册关联：{}/{} 个格式", status.registered, total)
    }
}

const SEEK_STEP: f64 = 5.0;
const VOLUME_STEP: f64 = 5.0;
/// 倍速菜单里的固定档位。自定义滑块的量程比它宽，两边共用同一套显示格式。
const SPEED_PRESETS: [f64; 9] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0];
/// 自定义倍速滑块的量程，与面板上标出的 0.1–5.0 一致。
const SPEED_MIN: f64 = 0.1;
const SPEED_MAX: f64 = 5.0;
/// 滑块的最小步进：这个宽度下拖到 0.01 也选不准，0.05 已经够用且数字好读。
const SPEED_STEP: f64 = 0.05;
/// 触控板走的是像素增量，攒够这么远算「一格」，走一帧。
const FRAME_SCROLL_PX: f32 = 48.0;
const CONTROLS_IDLE: Duration = Duration::from_millis(2600);
const OSD_LIFETIME: Duration = Duration::from_millis(900);
/// 截图卡片的宽度与预览区高度。
const SCREENSHOT_CARD_WIDTH: f32 = 280.0;
const SCREENSHOT_PREVIEW_HEIGHT: f32 = 158.0;
/// 没人搭理时多久自动收起。鼠标停在卡片上时不计时。
const SCREENSHOT_CARD_LIFETIME: Duration = Duration::from_secs(10);
const MENUBAR_HEIGHT: f32 = 32.0;
/// 播放列表面板拖动时的宽度边界。视频那边至少留 480px：再窄控制条上的
/// 倍速/循环/全屏就挤成一团，拖到头也不该出现这种布局。
const PLAYLIST_MIN_WIDTH: f32 = 220.0;
const PLAYLIST_MAX_WIDTH: f32 = 760.0;
const VIDEO_MIN_WIDTH: f32 = 480.0;
const PLAYLIST_RESIZER_WIDTH: f32 = 6.0;
/// 行右键菜单的尺寸，只用来把弹窗往窗口里收。
const ITEM_MENU_WIDTH: f32 = 168.0;
const ITEM_MENU_HEIGHT: f32 = 118.0;

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
    show_help: bool,
    /// 悬浮菜单（字幕 / 音轨 / 循环）当前展开的是哪一个。
    menu: Option<Menu>,
    /// 顶部菜单栏当前展开的是哪一个。
    menu_bar: Option<BarMenu>,
    /// 文件关联的落点缓存。
    ///
    /// 查询要逐个读注册表，不能放进每帧都跑的渲染路径里；只在展开「设置」菜单、
    /// 或者刚改过关联时刷新一次。
    association_status: platform::associations::Status,

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
    TogglePlaylist,
    ToggleFullscreen,
    ToggleHelp,
    TakeScreenshot,
    SetAsDefaultPlayer,
    OpenDefaultApps,
    RemoveAssociations,
    ToggleMaintainAssociations,
    About,
    Quit,
}

impl PlayerApp {
    pub fn new(options: LaunchOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = Settings::load();
        let focus = cx.focus_handle();

        let (event_tx, event_rx) = futures::channel::mpsc::unbounded();
        let (engine, startup_failure) = match Engine::spawn(event_tx) {
            Ok(engine) => (Some(Arc::new(engine)), None),
            Err(error) => {
                log_error!("初始化播放引擎失败: {error:#}");
                (None, Some(format!("{error:#}")))
            }
        };

        let mut app = Self {
            engine,
            snapshot: Snapshot {
                volume: settings.volume,
                muted: settings.muted,
                speed: settings.speed,
                ..Snapshot::default()
            },
            frame: None,
            incoming: None,
            retired: VecDeque::new(),
            playlists: PlaylistSet::load(),
            playing_tab: None,
            selection: BTreeSet::new(),
            selection_anchor: None,
            playlist_dragging: false,
            item_menu: None,
            file_info: None,
            settings,
            focus,
            last_activity: Instant::now(),
            controls_forced: true,
            dragging: None,
            progress_bounds: Rc::new(Cell::new(Bounds::default())),
            volume_bounds: Rc::new(Cell::new(Bounds::default())),
            volume_dragging: false,
            speed_bounds: Rc::new(Cell::new(Bounds::default())),
            speed_dragging: false,
            frame_scroll: 0.0,
            hover: None,
            preview_request: None,
            preview_retired: VecDeque::new(),
            dialog_open: false,
            osd: None,
            screenshot_card: None,
            error: None,
            show_help: false,
            menu: None,
            menu_bar: None,
            association_status: platform::associations::Status::default(),
            startup_failure,
        };

        // 应用保存的音量/静音/速度。
        if app.engine.is_some() {
            let volume = app.settings.volume;
            let muted = app.settings.muted;
            let speed = app.settings.speed;
            app.send(Command::SetVolume(volume));
            app.send(Command::SetMuted(muted));
            if (speed - 1.0).abs() > f64::EPSILON {
                app.send(Command::SetSpeed(speed));
            }
            // 软件渲染下 hwdec 会把帧从显存倒回内存，反而更慢；默认关闭。
            app.send(Command::SetOption {
                name: "hwdec".into(),
                value: app.settings.hwdec.clone(),
            });
        }

        app.consume_events(event_rx, cx);
        app.start_idle_watchdog(cx);
        app.auto_maintain_associations();

        if options.paths.is_empty() {
            app.restore_session();
        } else {
            app.open_paths(options.paths, cx);
        }
        if options.fullscreen {
            window.toggle_fullscreen();
        }

        app.focus.focus(window, cx);
        app
    }

    // -- 引擎交互 ----------------------------------------------------------

    fn send(&self, command: Command) {
        if let Some(engine) = &self.engine {
            engine.send(command);
        }
    }

    // -- 列表访问 ----------------------------------------------------------

    /// 当前显示页的列表。读操作一律走这里。
    fn playlist(&self) -> &Playlist {
        self.playlists.active_playlist()
    }

    fn playlist_mut(&mut self) -> &mut Playlist {
        self.playlists.active_playlist_mut()
    }

    /// 正在播的那一页。`None` 表示引擎里可能还有东西在放，但已经没有播放队列了
    /// （正在播的列表被关掉、清空或删光）。这时换集和自动续播都停下来，
    /// 等用户重新从列表里点一个文件——列表的增删合闭永远不打断播放。
    fn playback_tab(&self) -> Option<usize> {
        self.playing_tab
    }

    /// 面板宽度：跟着窗口收，避免窗口缩小后面板把画面整块吃掉。
    fn playlist_width(&self, window: &Window) -> f32 {
        let available = f32::from(window.viewport_size().width);
        let max = (available - VIDEO_MIN_WIDTH).clamp(PLAYLIST_MIN_WIDTH, PLAYLIST_MAX_WIDTH);
        self.settings.playlist_width.clamp(PLAYLIST_MIN_WIDTH, max)
    }

    /// 拖动面板左边界。面板贴右边，所以宽度就是「窗口宽 - 鼠标 x」。
    fn drag_playlist_width(&mut self, x: Pixels, window: &Window) {
        let available = f32::from(window.viewport_size().width);
        let max = (available - VIDEO_MIN_WIDTH).clamp(PLAYLIST_MIN_WIDTH, PLAYLIST_MAX_WIDTH);
        self.settings.playlist_width = (available - f32::from(x)).clamp(PLAYLIST_MIN_WIDTH, max);
    }

    fn consume_events(&self, mut receiver: UnboundedReceiver<PlayerEvent>, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            while let Some(event) = receiver.next().await {
                let alive = this.update(cx, |this, cx| match event {
                    PlayerEvent::Dirty => {
                        if let Some(engine) = &this.engine {
                            this.snapshot = engine.snapshot();
                        }
                        cx.notify();
                    }
                    PlayerEvent::Frame(image) => {
                        if let Some(engine) = &this.engine {
                            // 告诉取帧线程「这一帧已被取走」，否则它会主动丢帧。
                            engine.frame_consumed();
                        }
                        this.incoming = Some(image);
                        cx.notify();
                    }
                    PlayerEvent::Thumbnail { time, image } => {
                        this.on_thumbnail(time, image);
                        cx.notify();
                    }
                    PlayerEvent::Ended(reason) => {
                        this.on_ended(reason, cx);
                    }
                    PlayerEvent::Error(message) => {
                        log_warn!("播放器错误: {message}");
                        this.error = Some(Banner::new("播放出错", message));
                        cx.notify();
                    }
                    PlayerEvent::Screenshot(result) => {
                        match result {
                            Ok(path) => {
                                log_info!("截图已保存: {}", path.display());
                                // 用卡片代替 OSD：这里还要让用户能复制、打开、定位。
                                this.show_screenshot_card(path, cx);
                            }
                            Err(message) => {
                                log_warn!("截图失败: {message}");
                                this.flash(format!("截图失败：{}", error_reason(&message)));
                            }
                        }
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// 控制条自动隐藏：不用「每帧检查」，而是低频轮询，省得每次鼠标移动都
    /// 重新起一个定时器。
    fn start_idle_watchdog(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
                let alive = this.update(cx, |this, cx| {
                    let mut dirty = false;
                    if this
                        .osd
                        .as_ref()
                        .is_some_and(|(_, at)| at.elapsed() > OSD_LIFETIME)
                    {
                        this.osd = None;
                        dirty = true;
                    }
                    if this.controls_visible() && this.last_activity.elapsed() > CONTROLS_IDLE {
                        this.controls_forced = false;
                        dirty = true;
                    }
                    // 鼠标停在卡片上时不算超时：用户正要点按钮。
                    if this.screenshot_card.as_ref().is_some_and(|card| {
                        !card.hovered && card.shown_at.elapsed() > SCREENSHOT_CARD_LIFETIME
                    }) {
                        this.clear_screenshot_card();
                        dirty = true;
                    }
                    if dirty {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    // -- 播放控制 ----------------------------------------------------------

    fn controls_visible(&self) -> bool {
        self.controls_forced
            || self.last_activity.elapsed() < CONTROLS_IDLE
            // 暂停时一直留着：播放器停下的时候，用户的第一反应就是去找「播放」按钮，
            // 这时候把控制条藏起来等于让人以为界面上什么都没有。
            || self.snapshot.paused
            || self.menu.is_some()
            || self.menu_bar.is_some()
            || self.dragging.is_some()
            || self.volume_dragging
            || self.show_help
            || self.playlist().is_empty()
    }

    fn poke(&mut self) {
        self.last_activity = Instant::now();
    }

    fn toggle_help(&mut self, cx: &mut Context<Self>) {
        self.show_help = !self.show_help;
        cx.notify();
    }

    /// 播放列表面板的开合。关掉时顺手清掉选择：再打开时不该有一批
    /// 「关着的时候选的」行还亮着，Delete 也不该删到看不见的东西。
    fn set_playlist_visible(&mut self, visible: bool) {
        self.settings.show_playlist = visible;
        if !visible {
            self.selection.clear();
            self.selection_anchor = None;
        }
    }

    fn toggle_play(&mut self, cx: &mut Context<Self>) {
        self.send(Command::PlayPause);
        self.poke();
        cx.notify();
    }

    fn seek_by(&mut self, delta: f64, cx: &mut Context<Self>) {
        self.send(Command::SeekRelative(delta));
        let arrow = if delta > 0.0 { "⏩" } else { "⏪" };
        self.flash(format!("{arrow} {:.0}s", delta.abs()));
        cx.notify();
    }

    fn change_volume(&mut self, delta: f64, cx: &mut Context<Self>) {
        let next = (self.snapshot.volume + delta).clamp(0.0, 130.0);
        self.snapshot.volume = next;
        self.settings.volume = next;
        if self.snapshot.muted && next > 0.0 {
            self.snapshot.muted = false;
            self.send(Command::SetMuted(false));
        }
        self.send(Command::SetVolume(next));
        self.flash(format!("音量 {next:.0}%"));
        cx.notify();
    }

    /// 逐帧步进。`steps` 正数向前、负数向后。
    ///
    /// 看单帧时用户要的是「停在这一帧」，所以第一次滚轮就顺手暂停；本来就
    /// 暂停着则保持，不做 toggle，免得滚一下来回切。
    fn step_frame(&mut self, steps: i32, cx: &mut Context<Self>) {
        if self.engine.is_none() || self.snapshot.idle {
            return;
        }
        if !self.snapshot.paused {
            self.snapshot.paused = true;
            self.send(Command::SetPaused(true));
        }
        self.send(Command::StepFrame(steps));
        let arrow = if steps > 0 { "⏭" } else { "⏮" };
        self.flash(format!("{arrow} 逐帧"));
        cx.notify();
    }

    fn set_volume(&mut self, volume: f64) {
        let next = volume.clamp(0.0, 130.0);
        self.snapshot.volume = next;
        self.send(Command::SetVolume(next));
    }

    fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        let muted = !self.snapshot.muted;
        self.snapshot.muted = muted;
        self.send(Command::SetMuted(muted));
        self.flash(if muted { "已静音" } else { "取消静音" });
        cx.notify();
    }

    fn set_speed(&mut self, speed: f64, cx: &mut Context<Self>) {
        let next = (speed * 100.0).round() / 100.0;
        self.snapshot.speed = next;
        self.settings.speed = next;
        self.send(Command::SetSpeed(next));
        self.flash(format!("{next:.2}× 速度"));
        cx.notify();
    }

    /// 把自定义滑块的横坐标换算成倍速。
    ///
    /// 拖动过程里不弹 OSD：数字就在面板的气泡上，再盖一层反而挡住画面。
    fn set_speed_from_x(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let bounds = self.speed_bounds.get();
        let width = f32::from(bounds.size.width) as f64;
        if width <= 1.0 {
            return;
        }
        let offset = (f32::from(x) - f32::from(bounds.origin.x)) as f64;
        let ratio = (offset / width).clamp(0.0, 1.0);
        let steps = (ratio * (SPEED_MAX - SPEED_MIN) / SPEED_STEP).round();
        let speed = (SPEED_MIN + steps * SPEED_STEP).clamp(SPEED_MIN, SPEED_MAX);
        let next = (speed * 100.0).round() / 100.0;
        // 鼠标在同一个像素里抖动时别反复给 mpv 发同一条命令。
        if (self.snapshot.speed - next).abs() < 0.001 {
            return;
        }
        self.snapshot.speed = next;
        self.settings.speed = next;
        self.send(Command::SetSpeed(next));
        cx.notify();
    }

    fn set_loop(&mut self, mode: LoopMode, cx: &mut Context<Self>) {
        self.settings.loop_mode = mode;
        self.flash(mode.label());
        cx.notify();
    }

    /// 让 mpv 抓当前画面存成 PNG。
    ///
    /// 不用界面里那一帧：它是按窗口尺寸缩过的，存档要的是原始分辨率。
    fn take_screenshot(&mut self, cx: &mut Context<Self>) {
        if self.engine.is_none() {
            self.flash("截图不可用：播放引擎没有加载");
            cx.notify();
            return;
        }
        if !self.snapshot.has_video() {
            self.flash("没有可截图的画面");
            cx.notify();
            return;
        }

        let video = self.snapshot.path.clone();
        let target = match crate::screenshot::target(video.as_deref(), self.snapshot.position) {
            Ok(path) => path,
            Err(error) => {
                log_warn!("准备截图失败: {error:#}");
                self.flash(format!("截图失败：{error}"));
                cx.notify();
                return;
            }
        };
        self.send(Command::Screenshot(target.to_string_lossy().into_owned()));
        cx.notify();
    }

    /// 截图成功后弹出操作卡片，顺手在后台把截图缩成预览图。
    fn show_screenshot_card(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        // 上一张卡片的缩略图已经进过图集，得走同一条回收路径，不能直接丢。
        self.clear_screenshot_card();
        self.screenshot_card = Some(ScreenshotCard {
            path: path.clone(),
            thumb: ScreenshotThumb::Loading,
            shown_at: Instant::now(),
            hovered: false,
        });

        // 解码要几十毫秒（4K 截图更久），放在主线程上会明显卡一下。
        let (tx, rx) = futures::channel::oneshot::channel();
        let target = path.clone();
        std::thread::spawn(move || {
            let _ = tx.send(crate::screenshot::thumbnail(
                &target,
                crate::screenshot::THUMBNAIL_MAX_WIDTH,
            ));
        });

        cx.spawn(async move |this, cx| {
            let thumb = match rx.await {
                Ok(Ok(image)) => ScreenshotThumb::Ready(image),
                Ok(Err(error)) => {
                    log_warn!("读取截图预览失败: {error:#}");
                    ScreenshotThumb::Failed
                }
                Err(_) => ScreenshotThumb::Failed,
            };
            let _ = this.update(cx, |this, cx| {
                // 解码期间可能又截了一张，或者卡片已经关掉：只认同一份。
                let Some(card) = this.screenshot_card.as_mut() else {
                    return;
                };
                if card.path != path {
                    return;
                }
                card.thumb = thumb;
                cx.notify();
            });
        })
        .detach();
    }

    /// 收起截图卡片。缩略图不能直接丢：它已经进过 GPU 图集，要交给渲染路径归还。
    fn clear_screenshot_card(&mut self) {
        if let Some(card) = self.screenshot_card.take()
            && let ScreenshotThumb::Ready(thumb) = card.thumb
        {
            self.preview_retired.push_back(thumb);
        }
    }

    fn copy_screenshot(&mut self, cx: &mut Context<Self>) {
        let Some(card) = &self.screenshot_card else {
            return;
        };
        let path = card.path.clone();
        match platform::clipboard::item(&path) {
            Ok(item) => {
                cx.write_to_clipboard(item);
                self.flash(platform::clipboard::COPIED_MESSAGE);
            }
            Err(error) => self.flash(format!("复制截图失败：{error:#}")),
        }
        cx.notify();
    }

    fn open_screenshot(&mut self, cx: &mut Context<Self>) {
        let Some(card) = &self.screenshot_card else {
            return;
        };
        let path = card.path.clone();
        match platform::open::open(&path) {
            // 默认程序已经把窗口抢走了，卡片留着只会挡在新窗口后面。
            Ok(()) => self.clear_screenshot_card(),
            Err(message) => self.flash(message),
        }
        cx.notify();
    }

    fn reveal_screenshot(&mut self, cx: &mut Context<Self>) {
        let Some(card) = &self.screenshot_card else {
            return;
        };
        let path = card.path.clone();
        match platform::reveal::reveal(&path) {
            Ok(()) => self.clear_screenshot_card(),
            Err(message) => self.flash(message),
        }
        cx.notify();
    }

    fn toggle_fullscreen(&mut self, window: &mut Window) {
        window.toggle_fullscreen();
    }

    /// Esc 的语义：一层一层往回退，而不是直接全关。
    fn exit_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.show_help {
            self.show_help = false;
        } else if self.file_info.is_some() {
            self.file_info = None;
        } else if self.item_menu.is_some() {
            self.item_menu = None;
        } else if self.menu_bar.is_some() {
            self.menu_bar = None;
        } else if self.menu == Some(Menu::SpeedCustom) {
            // 自定义面板是倍速菜单的第二层，Esc 先退回预设列表。
            self.menu = Some(Menu::Speed);
        } else if self.menu.is_some() {
            self.menu = None;
        } else if self.screenshot_card.is_some() {
            // 卡片是最后一个浮上来的东西，Esc 先收它，再考虑关面板、退全屏。
            self.clear_screenshot_card();
        } else if self.settings.show_playlist {
            self.set_playlist_visible(false);
        } else if window.is_fullscreen() {
            window.toggle_fullscreen();
        }
        self.poke();
        cx.notify();
    }

    fn flash(&mut self, message: impl Into<SharedString>) {
        let text = message.into();
        self.osd = Some((
            text.split('\n').map(|line| line.to_string()).collect(),
            Instant::now(),
        ));
    }

    fn on_ended(&mut self, reason: EndReason, cx: &mut Context<Self>) {
        // `Stopped` 是「被 stop、或正在播的文件被新文件顶掉」，不是放完了。
        // 换歌 / 删除当前项都会顺带触发它，跟着往前走会白跳一个文件。
        if matches!(
            reason,
            EndReason::Error | EndReason::Stopped | EndReason::Quit
        ) {
            cx.notify();
            return;
        }
        // 自动续播跟着「正在播的那一页」走，不跟当前显示页：否则在列表 B 里
        // 翻看时，A 里的片子放完了会跳到 B 的队列里。
        let Some(tab) = self.playback_tab() else {
            // 队列被关掉/清空了，就让这一集自然放完，不再往后接。
            self.controls_forced = true;
            cx.notify();
            return;
        };
        match self.settings.loop_mode {
            LoopMode::One => {
                if let Some(path) = self.playlists.tab(tab).and_then(|p| p.current()).cloned() {
                    self.load_file(&path);
                }
            }
            LoopMode::All => {
                if let Some(next) = self.playlists.tab(tab).and_then(|p| p.next_index(true)) {
                    self.play_tab_index(tab, next);
                } else {
                    self.controls_forced = true;
                    cx.notify();
                }
            }
            LoopMode::Off => {
                if let Some(next) = self.playlists.tab(tab).and_then(|p| p.next_index(false)) {
                    self.play_tab_index(tab, next);
                } else {
                    self.controls_forced = true;
                    cx.notify();
                }
            }
        }
    }

    // -- 列表与打开 --------------------------------------------------------

    fn open_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let mut files: Vec<PathBuf> = Vec::new();
        let mut focus: Option<PathBuf> = None;
        // 单独打开一个 .m3u/.pls 时新开一页：它本来就是一份独立列表，
        // 直接覆盖当前页会把用户正在看的那一页顶掉。
        let as_new_tab = paths.len() == 1 && formats::is_playlist(&paths[0]);
        let label = tab_name_for(&paths);

        for path in paths {
            if path.is_dir() {
                let mut collected = playlist::collect_dir(&path);
                focus = focus.or_else(|| collected.first().cloned());
                files.append(&mut collected);
            } else if formats::is_playlist(&path) {
                let mut entries = playlist::parse_playlist(&path);
                focus = focus.or_else(|| entries.first().cloned());
                files.append(&mut entries);
            } else if formats::is_subtitle(&path) {
                if let Some(engine) = &self.engine {
                    engine.open_subtitle(&path);
                    self.flash(format!(
                        "已加载字幕：{}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ));
                }
                cx.notify();
                return;
            } else {
                files.push(path.clone());
            }
        }

        if files.is_empty() {
            self.flash("没有找到可播放的文件");
            cx.notify();
            return;
        }

        // 只拖进来一个文件时，把同目录的其它媒体一起排队 —— 看剧集时这是默认期待。
        let single = files.len() == 1;
        if single {
            let path = files[0].clone();
            focus = Some(path.clone());
            if let Some(parent) = path.parent() {
                let mut siblings = playlist::collect_dir(parent);
                if siblings.len() > 1 {
                    files = std::mem::take(&mut siblings);
                }
            }
        }

        let start = focus
            .as_ref()
            .and_then(|target| files.iter().position(|path| path == target))
            .unwrap_or(0);

        if let Some(path) = &focus {
            self.settings.last_dir = path.parent().map(|p| p.to_path_buf());
        }

        // 当前页本来就是空的就别再新开一页，否则启动时那个空页会一直留着。
        let reuse_empty = self.playlist().is_empty();
        if as_new_tab && !reuse_empty {
            let name = label.unwrap_or_else(|| self.playlists.auto_name());
            self.playlists.add(name);
        } else if reuse_empty {
            // 空页借这次打开的文件起个名字，省得列表全叫「列表 N」。
            if let Some(name) = label {
                self.playlists.rename_active(name);
            }
        }
        self.playlist_mut().replace(files, start);
        self.selection.clear();
        self.selection_anchor = None;
        self.play_index(start);
    }

    /// 播放**当前显示页**里的第 `index` 项。
    fn play_index(&mut self, index: usize) {
        let tab = self.playlists.active();
        self.play_tab_index(tab, index);
    }

    /// 播放某一页里的第 `index` 项：页内位置、播放归属、引擎三者一起对齐。
    fn play_tab_index(&mut self, tab: usize, index: usize) {
        {
            let Some(playlist) = self.playlists.tab_mut(tab) else {
                return;
            };
            playlist.set_index(index);
        }
        self.playing_tab = Some(tab);
        if let Some(path) = self.playlists.tab(tab).and_then(|p| p.get(index)).cloned() {
            self.load_file(&path);
        }
        self.playlists.save();
    }

    /// 启动时恢复上次的列表。
    ///
    /// 装载当前项但先按下暂停：用户可能只是顺手点开了播放器，不该一上来就出声。
    fn restore_session(&mut self) {
        let Some(path) = self.playlist().current().cloned() else {
            return;
        };
        if !path.exists() {
            // 文件可能在被拔掉的移动硬盘上：列表留着，但不在启动时糊一条错误横幅。
            log_warn!("上次的播放项已不存在，跳过自动装载: {}", path.display());
            return;
        }
        self.playing_tab = Some(self.playlists.active());
        self.load_file(&path);
        self.send(Command::SetPaused(true));
    }

    fn load_file(&mut self, path: &std::path::Path) {
        let url = path.to_string_lossy().to_string();
        self.snapshot.position = 0.0;
        self.snapshot.duration = 0.0;
        self.snapshot.tracks.clear();
        self.error = None;
        // 换片：上一部的预览图和时间点全部作废，预览实例里那个文件也卸掉，
        // 否则它会一直占着文件和一路解码器。
        self.clear_preview();
        self.send(Command::Load { url, start: None });
        log_info!("打开 {}", path.display());
    }

    fn next_file(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.playback_tab() else {
            return;
        };
        let wrap = self.settings.loop_mode != LoopMode::Off
            || self.playlists.tab(tab).is_some_and(|list| list.len() > 1);
        if let Some(index) = self
            .playlists
            .tab(tab)
            .and_then(|list| list.next_index(wrap))
        {
            self.play_tab_index(tab, index);
            cx.notify();
        }
    }

    fn previous_file(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.playback_tab() else {
            return;
        };
        let wrap = self.settings.loop_mode != LoopMode::Off
            || self.playlists.tab(tab).is_some_and(|list| list.len() > 1);
        if let Some(index) = self
            .playlists
            .tab(tab)
            .and_then(|list| list.previous_index(wrap))
        {
            self.play_tab_index(tab, index);
            cx.notify();
        }
    }

    // -- 列表编辑 ----------------------------------------------------------

    /// 列表行的单击语义：普通点击播放，Ctrl/⌘ 点击增删选择，Shift 点击连选。
    ///
    /// 没有单独的「选择模式」开关——播放器里点列表的第一意图永远是换歌，
    /// 把多选藏在修饰键后面比让每次点击都先进选择态更顺手。
    fn select_item(&mut self, index: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        if modifiers.shift {
            let anchor = self.selection_anchor.unwrap_or(index);
            let (start, end) = if anchor <= index {
                (anchor, index)
            } else {
                (index, anchor)
            };
            // 连选替换上一次的区间，否则反复 Shift 点击只会越选越多、缩不回去。
            self.selection = (start..=end).collect();
            if self.selection_anchor.is_none() {
                self.selection_anchor = Some(index);
            }
            self.poke();
            cx.notify();
            return;
        }

        if modifiers.secondary() {
            if !self.selection.remove(&index) {
                self.selection.insert(index);
            }
            self.selection_anchor = Some(index);
            self.poke();
            cx.notify();
            return;
        }

        self.selection.clear();
        self.selection_anchor = Some(index);
        self.play_index(index);
        self.poke();
        cx.notify();
    }

    /// 删除当前页里的若干项。
    ///
    /// 只改列表，不动引擎：正在放的文件早就装进去解码了，删掉列表里的条目不该让
    /// 画面黑一下。页内位置由 `remove_many` 顺延到后继项，这一集放完从那里接着走。
    fn remove_items(&mut self, indexes: &[usize], cx: &mut Context<Self>) {
        let tab = self.playlists.active();
        let removal = {
            let Some(list) = self.playlists.tab_mut(tab) else {
                return;
            };
            list.remove_many(indexes)
        };
        if removal.removed == 0 {
            return;
        }
        self.selection.clear();
        self.selection_anchor = None;
        self.playlists.save();
        self.flash(format!("已移除 {} 个文件", removal.removed));
        self.poke();
        cx.notify();
    }

    fn remove_selection(&mut self, cx: &mut Context<Self>) {
        if self.selection.is_empty() {
            return;
        }
        let indexes: Vec<usize> = self.selection.iter().copied().collect();
        self.remove_items(&indexes, cx);
    }

    fn clear_playlist(&mut self, cx: &mut Context<Self>) {
        let count = self.playlist().len();
        if count == 0 {
            return;
        }
        // 只清列表，播放和磁盘上的文件都不动，所以不做二次确认——重新拖进来就行。
        self.playlist_mut().clear();
        self.selection.clear();
        self.selection_anchor = None;
        self.playlists.save();
        self.flash(format!("已清空播放列表（{count} 个文件仍在磁盘上）"));
        cx.notify();
    }

    // -- 多列表 ------------------------------------------------------------

    fn add_tab(&mut self, cx: &mut Context<Self>) {
        let name = self.playlists.auto_name();
        self.playlists.add(name);
        self.selection.clear();
        self.selection_anchor = None;
        self.playlists.save();
        self.poke();
        cx.notify();
    }

    /// 切页不动播放：面板同时兼任「浏览」的职责，切过去就换歌太打扰。
    fn select_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        self.playlists.set_active(index);
        self.selection.clear();
        self.selection_anchor = None;
        self.playlists.save();
        self.poke();
        cx.notify();
    }

    fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        let name = self
            .playlists
            .tabs()
            .get(index)
            .map(|tab| tab.name.clone())
            .unwrap_or_default();
        if !self.playlists.remove(index) {
            return;
        }
        // 关页面不动播放：正在放的东西还在引擎里，让它自然放完，只是不再有「下一集」。
        if self.playing_tab == Some(index) {
            self.playing_tab = None;
        } else if let Some(playing) = self.playing_tab
            && playing > index
        {
            self.playing_tab = Some(playing - 1);
        }
        self.selection.clear();
        self.selection_anchor = None;
        self.playlists.save();
        self.flash(format!("已关闭列表「{name}」"));
        self.poke();
        cx.notify();
    }

    // -- 行右键菜单 --------------------------------------------------------

    /// 右键某一行：在鼠标位置弹出菜单。越界就往窗口里收，别飘到屏幕外。
    fn open_item_menu(
        &mut self,
        index: usize,
        path: PathBuf,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let viewport = window.viewport_size();
        let max_x = (f32::from(viewport.width) - ITEM_MENU_WIDTH - 8.0).max(8.0);
        let max_y = (f32::from(viewport.height) - ITEM_MENU_HEIGHT - 8.0).max(8.0);
        let position = point(
            px(f32::from(position.x).clamp(8.0, max_x)),
            px(f32::from(position.y).clamp(8.0, max_y)),
        );

        self.item_menu = Some(ItemMenu {
            index,
            path,
            position,
        });
        // 右键菜单和悬浮菜单/菜单栏互斥，免得两层浮层叠在一起。
        self.menu = None;
        self.menu_bar = None;
        self.poke();
        cx.notify();
    }

    fn reveal_item(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.item_menu = None;
        match platform::reveal::reveal(&path) {
            // 文件管理器可能被别的窗口盖住，给一句提示免得以为没反应。
            Ok(()) => self.flash("已在文件管理器中定位"),
            Err(message) => self.flash(message),
        }
        cx.notify();
    }

    /// 打开「查看格式」。探测要 loadfile，放到后台线程里跑，结果用 oneshot 回来。
    fn show_file_info(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.item_menu = None;
        self.file_info = Some(FileInfoPanel {
            path: path.clone(),
            state: InfoState::Loading,
        });

        let target = path.clone();
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::player::probe::probe(&target));
        });

        cx.spawn(async move |this, cx| {
            let result = rx
                .await
                .unwrap_or_else(|_| Err("探测线程意外退出".to_string()));
            let _ = this.update(cx, |this, cx| {
                // 探测期间面板可能被关掉，或者已经换成另一个文件：只认同一份。
                let Some(panel) = this.file_info.as_mut() else {
                    return;
                };
                if panel.path != path {
                    return;
                }
                panel.state = match result {
                    Ok(info) => InfoState::Ready(info),
                    Err(message) => InfoState::Failed(message),
                };
                cx.notify();
            });
        })
        .detach();

        cx.notify();
    }

    fn pick_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let (label, extensions) = formats::dialog_filter();
        // 挂到主窗口上：对话框是模态的，打开期间主窗口不接受输入。
        // 过滤器给的是裸扩展名，`rfd` 自己补 `*.`；另外留一个「所有文件」，
        // 这样清单里没有的格式也还能手动翻出来打开（mpv 认得的多得多）。
        let mut dialog = rfd::FileDialog::new()
            .add_filter(&label, &extensions)
            .add_filter("视频文件", formats::VIDEO)
            .add_filter("音频文件", formats::AUDIO)
            .add_filter("所有文件", &["*"])
            .set_parent(window);
        if let Some(dir) = &self.settings.last_dir {
            dialog = dialog.set_directory(dir);
        }

        // rfd 的对话框是阻塞的：扔到独立线程，结果通过 oneshot 回来。
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(dialog.pick_files());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten().unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if picked.is_empty() {
                    return;
                }
                this.open_paths(picked, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let mut dialog = rfd::FileDialog::new().set_parent(window);
        if let Some(dir) = &self.settings.last_dir {
            dialog = dialog.set_directory(dir);
        }
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(dialog.pick_folder());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if let Some(folder) = picked {
                    this.open_paths(vec![folder], cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn pick_subtitle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let mut dialog = rfd::FileDialog::new()
            .add_filter("字幕文件", formats::SUBTITLE)
            .add_filter("所有文件", &["*"])
            .set_parent(window);
        if let Some(path) = self.playlist().current() {
            if let Some(dir) = path.parent() {
                dialog = dialog.set_directory(dir);
            }
        }
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(dialog.pick_file());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if let Some(path) = picked {
                    let url = path.to_string_lossy().to_string();
                    this.send(Command::AddSubtitle(url));
                    this.flash(format!(
                        "已加载字幕：{}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    // -- 帧 ----------------------------------------------------------------

    fn swap_frame(&mut self, window: &mut Window) {
        let Some(incoming) = self.incoming.take() else {
            return;
        };
        if let Some(previous) = self.frame.replace(incoming) {
            self.retired.push_back(previous);
            while self.retired.len() > 2 {
                if let Some(stale) = self.retired.pop_front() {
                    let _ = window.drop_image(stale);
                }
            }
        }
    }

    /// 把不再显示的预览图还给 atlas。
    ///
    /// `drop_image` 只应该在没有元素再画它的时候调，所以在渲染路径上做，
    /// 而且留一张余量——刚换下来的那张可能还在 GPU 队列里被采样。
    fn drop_retired_previews(&mut self, window: &mut Window) {
        while self.preview_retired.len() > 1 {
            if let Some(stale) = self.preview_retired.pop_front() {
                let _ = window.drop_image(stale);
            }
        }
    }

    /// 告诉取帧线程应该按多大尺寸渲染。
    ///
    /// 传的是**整个视频区域**的物理像素（不是等比适配后的画面矩形）：mpv 在给定
    /// 尺寸里自己按视频比例排黑边，我们再把这个结果铺满区域。这样即使某一帧的
    /// 宽高比信息还没跟上（刚切歌），画面也不会被拉伸，最多黑边宽度短暂不对。
    ///
    /// 区域比视频原始分辨率大时按原始分辨率渲染：放大不增加细节，只是让每帧
    /// 多拷几 MB。
    fn update_render_target(&self, window: &mut Window) {
        let Some(engine) = &self.engine else {
            return;
        };
        let viewport = window.viewport_size();
        let scale = window.scale_factor();
        // 面板占掉的那一条不算视频区域，否则缩画面时会被拉伸。
        let panel = if self.settings.show_playlist {
            self.playlist_width(window)
        } else {
            0.0
        };
        let area_width = (f32::from(viewport.width) - panel).max(2.0) * scale;
        // 窗口模式下菜单栏占掉了顶部一条，渲染目标要扣掉，否则画面会被裁掉一截。
        let chrome = if window.is_fullscreen() {
            0.0
        } else {
            MENUBAR_HEIGHT
        };
        let area_height = (f32::from(viewport.height) - chrome) * scale;
        if area_width < 2.0 || area_height < 2.0 {
            return;
        }

        let video_width = self.snapshot.video_width as f32;
        let video_height = self.snapshot.video_height as f32;
        let fit = if video_width > 0.0 && video_height > 0.0 {
            (video_width / area_width)
                .min(video_height / area_height)
                .min(1.0)
        } else {
            1.0
        };

        let width = (area_width * fit).round().max(2.0) as u32;
        let height = (area_height * fit).round().max(2.0) as u32;
        engine.set_render_target(width, height);
    }

    fn progress_value(&self) -> f64 {
        self.dragging.unwrap_or(self.snapshot.position)
    }

    // -- 渲染 --------------------------------------------------------------

    fn render_video_area(
        &self,
        top: Pixels,
        panel: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id("video-area")
            .absolute()
            .inset_0()
            .top(top)
            // 面板是真正的侧边栏：画面让出它那一条，而不是被盖住。
            .when(panel > 0.0, |this| this.right(px(panel)))
            .bg(bg())
            .child(ui::video_surface(self.frame.clone()))
            // 空状态只看「有没有帧」，不看 mpv 上报的宽高：width/height 属性在
            // 视频解码前是 0，用它会在一段正常播放的画面上盖一层「正在加载」。
            .when(self.frame.is_none(), |this| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(14.0))
                        .child(icon(IconName::Film, 56.0, text_faint()))
                        .child(div().text_size(px(15.0)).text_color(text_muted()).child(
                            if self.playlist().is_empty() {
                                "拖入视频文件，或按 O 打开"
                            } else {
                                "正在加载…"
                            },
                        ))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(text_faint())
                                .child(format!(
                                    "支持 {} 种常见容器与编码 · 其余格式交给 libmpv 自动识别",
                                    formats::supported_extension_count()
                                )),
                        ),
                )
            })
            .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                // 双击全屏；单击切换播放。播放器里这是最不容易误触的默认组合。
                if event.click_count() >= 2 {
                    this.toggle_fullscreen(window);
                } else if this.menu.is_some() || this.menu_bar.is_some() {
                    this.menu = None;
                    this.menu_bar = None;
                } else {
                    this.toggle_play(cx);
                }
                cx.notify();
            }))
    }

    fn render_progress(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let duration = self.snapshot.duration.max(0.001);
        let position = self.progress_value().clamp(0.0, duration);
        let ratio = (position / duration).clamp(0.0, 1.0) as f32;
        let dragging = self.dragging.is_some();
        let slot = self.progress_bounds.clone();

        let bar = div()
            .relative()
            .h(px(if dragging { 6.0 } else { 4.0 }))
            .w_full()
            .rounded(px(3.0))
            .bg(gpui_kit::rgba(0xffffff30))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(gpui_kit::relative(ratio))
                    .rounded(px(3.0))
                    .bg(ui::accent())
                    .child(
                        div()
                            .absolute()
                            .right(px(-4.5))
                            .top(px(-3.5))
                            .size(px(if dragging { 13.0 } else { 11.0 }))
                            .rounded_full()
                            .bg(ui::accent()),
                    ),
            );

        div()
            .id("progress")
            .relative()
            .w_full()
            .h(px(18.0))
            .flex()
            .items_center()
            .cursor(CursorStyle::PointingHand)
            .child(
                canvas(
                    move |bounds, _window, _cx| slot.set(bounds),
                    |_bounds, (), _window, _cx| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(bar)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.begin_progress_drag(event.position.x);
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _window, cx| {
                    this.end_progress_drag(cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                this.update_hover(Some(event.position.x), cx);
            }))
            // 指针离开这一条只能靠 `on_hover` 得知：移出去之后这棵子树
            // 再也收不到鼠标事件，光靠 on_mouse_move 浮层会一直挂在那里。
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if !*hovered {
                    this.update_hover(None, cx);
                }
            }))
            .children(self.hover.as_ref().map(|hover| self.render_preview(hover)))
    }

    /// 进度条上方的悬停预览：一张小图 + 对应时间。
    ///
    /// 浮层是进度条的绝对定位子节点，画在画面之上、控制条之外，
    /// 不占控制条的布局空间，也不会把进度条顶高。
    fn render_preview(&self, hover: &HoverPreview) -> AnyElement {
        const WIDTH: f32 = 208.0;
        const PADDING: f32 = 3.0;
        const LABEL: f32 = 18.0;

        let bar_width = f32::from(self.progress_bounds.get().size.width);
        // 跟着指针走，但不许越出进度条两端——浮层跑到控制条外面看着像画错了。
        let left = (hover.ratio * bar_width - WIDTH * 0.5).clamp(0.0, (bar_width - WIDTH).max(0.0));

        // 图框高度按视频比例走：宽银幕片子不该被塞进一个 16:9 的框里。
        let aspect = self.snapshot.aspect().clamp(0.5, 4.0);
        let image_height = ((WIDTH - PADDING * 2.0) / aspect).clamp(60.0, 200.0);
        let image = hover.image.clone();
        let time = hover.time;

        div()
            .absolute()
            .left(px(left))
            .bottom(px(22.0))
            .w(px(WIDTH))
            .p(px(PADDING))
            .flex()
            .flex_col()
            .gap(px(PADDING))
            .rounded(px(8.0))
            .bg(surface())
            .border_1()
            .border_color(gpui_kit::rgb(0x2c2c34))
            .shadow_lg()
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(image_height))
                    .rounded(px(5.0))
                    .overflow_hidden()
                    .bg(gpui_kit::rgb(0x000000))
                    .child(
                        canvas(
                            |_bounds, _window, _cx| (),
                            move |bounds, (), window, _cx| {
                                if let Some(image) = image.as_ref() {
                                    let _ = window.paint_image(
                                        bounds,
                                        bounds,
                                        Corners::default(),
                                        image.clone(),
                                        0,
                                        false,
                                    );
                                }
                            },
                        )
                        .absolute()
                        .inset_0(),
                    ),
            )
            .child(
                div()
                    .h(px(LABEL))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.0))
                    .text_color(text())
                    .child(format_time(time)),
            )
            .into_any_element()
    }

    fn begin_progress_drag(&mut self, x: Pixels) {
        self.dragging = Some(self.position_for_x(x));
    }

    fn update_progress_drag(&mut self, x: Pixels) {
        if self.dragging.is_some() {
            self.dragging = Some(self.position_for_x(x));
        }
    }

    fn end_progress_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(position) = self.dragging.take() {
            self.send(Command::SeekAbsolute(position));
            self.poke();
            cx.notify();
        }
    }

    fn position_for_x(&self, x: Pixels) -> f64 {
        let bounds = self.progress_bounds.get();
        let width = f32::from(bounds.size.width) as f64;
        if width <= 1.0 || self.snapshot.duration <= 0.0 {
            return self.snapshot.position;
        }
        let offset = f32::from(x) - f32::from(bounds.origin.x);
        let ratio = (offset as f64 / width).clamp(0.0, 1.0);
        ratio * self.snapshot.duration
    }

    // -- 进度条悬停预览 ----------------------------------------------------

    /// 鼠标在进度条上（`Some(x)`）或离开（`None`）时更新预览浮层。
    fn update_hover(&mut self, x: Option<Pixels>, cx: &mut Context<Self>) {
        let Some(x) = x else {
            if self.hover.is_some() {
                self.drop_hover();
                cx.notify();
            }
            return;
        };

        let duration = self.snapshot.duration;
        let bounds = self.progress_bounds.get();
        let width = f32::from(bounds.size.width);
        if duration <= 0.0 || width <= 1.0 {
            return;
        }

        let offset = (f32::from(x) - f32::from(bounds.origin.x)).clamp(0.0, width);
        let ratio = offset / width;
        let time = ratio as f64 * duration;

        // 只有「手上没有挂着的请求」才发新的。结果回来后会按当时的位置再补一条，
        // 所以最后一格不会漏；而中途那条即使丢掉，用户也早就划过去了。
        // 纯音频没有画面可取，别让预览实例在那儿白解码。
        let pending = self
            .preview_request
            .is_some_and(|(_, at)| at.elapsed() < Duration::from_millis(1500));
        if !pending && self.snapshot.has_video() {
            self.request_preview(time);
        }

        let image = self.hover.as_ref().and_then(|hover| hover.image.clone());
        self.hover = Some(HoverPreview { time, ratio, image });
        cx.notify();
    }

    fn request_preview(&mut self, time: f64) {
        let Some(path) = self.snapshot.path.clone() else {
            return;
        };
        let Some(engine) = &self.engine else {
            return;
        };
        engine.preview(&path, time);
        self.preview_request = Some((time, Instant::now()));
    }

    /// 预览图回来了。
    fn on_thumbnail(&mut self, time: f64, image: Arc<RenderImage>) {
        self.preview_request = None;

        let Some(hover) = &mut self.hover else {
            // 鼠标已经离开进度条：图没地方放，但它在 atlas 里占着一块，
            // 交给渲染路径统一回收。
            self.preview_retired.push_back(image);
            return;
        };

        if let Some(previous) = hover.image.replace(image) {
            self.preview_retired.push_back(previous);
        }

        // 送来的是几格之前的位置：指针多半已经挪远了，按当前位置再要一张。
        // 鼠标停下后下一次就能对上，所以这个「追」不会无限循环。
        let follow_up = ((hover.time - time).abs() > 0.5).then_some(hover.time);
        if let Some(time) = follow_up {
            self.request_preview(time);
        }
    }

    /// 鼠标离开进度条：浮层收掉。
    ///
    /// 预览实例仍然留着那个文件——回头再划过进度条不用重新打开，
    /// 打开一个文件比 seek 一次贵得多。
    fn drop_hover(&mut self) {
        if let Some(hover) = self.hover.take()
            && let Some(image) = hover.image
        {
            self.preview_retired.push_back(image);
        }
        self.preview_request = None;
    }

    /// 换片/关文件：预览整体作废，连预览实例里的文件一起卸掉。
    fn clear_preview(&mut self) {
        self.drop_hover();
        if let Some(engine) = &self.engine {
            engine.stop_preview();
        }
    }

    fn render_volume(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = self.snapshot.muted;
        let volume = if muted { 0.0 } else { self.snapshot.volume };
        let ratio = (volume / 130.0).clamp(0.0, 1.0) as f32;
        let slot = self.volume_bounds.clone();

        div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .child(icon_button(
                "mute",
                if muted || volume <= 0.0 {
                    IconName::VolumeX
                } else if volume < 55.0 {
                    IconName::Volume1
                } else {
                    IconName::Volume2
                },
                muted,
                false,
                cx.listener(|this, _: &ClickEvent, _window, cx| this.toggle_mute(cx)),
            ))
            .child(
                div()
                    .id("volume")
                    .relative()
                    .w(px(84.0))
                    .h(px(18.0))
                    .flex()
                    .items_center()
                    .cursor(CursorStyle::PointingHand)
                    .child(
                        canvas(
                            move |bounds, _window, _cx| slot.set(bounds),
                            |_bounds, (), _window, _cx| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(
                        div()
                            .relative()
                            .h(px(4.0))
                            .w_full()
                            .rounded(px(2.0))
                            .bg(gpui_kit::rgb(0x3a3a44))
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .bottom_0()
                                    .w(gpui_kit::relative(ratio))
                                    .rounded(px(2.0))
                                    .bg(if muted { text_faint() } else { text() }),
                            ),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            cx.stop_propagation();
                            this.volume_dragging = true;
                            this.set_volume_from_x(event.position.x);
                            cx.notify();
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseUpEvent, _window, cx| {
                            this.volume_dragging = false;
                            this.settings.volume = this.snapshot.volume;
                            cx.notify();
                        }),
                    ),
            )
    }

    fn set_volume_from_x(&mut self, x: Pixels) {
        let bounds = self.volume_bounds.get();
        let width = f32::from(bounds.size.width) as f64;
        if width <= 1.0 {
            return;
        }
        let offset = f32::from(x) - f32::from(bounds.origin.x);
        let ratio = (offset as f64 / width).clamp(0.0, 1.0);
        let volume = ratio * 130.0;
        if self.snapshot.muted && volume > 0.0 {
            self.snapshot.muted = false;
            self.send(Command::SetMuted(false));
        }
        self.set_volume(volume);
    }

    fn render_controls(&self, panel: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = &self.snapshot;
        let playing = !snapshot.paused;
        let position = self.progress_value();
        let duration = snapshot.duration;
        let has_video = snapshot.has_video();

        let time_text = if duration > 0.0 {
            format!("{} / {}", format_time(position), format_time(duration))
        } else {
            format!("{} / --:--", format_time(position))
        };

        let subtitle_active = snapshot
            .selected_track(crate::player::TrackKind::Subtitle)
            .is_some();

        div()
            .id("controls")
            .absolute()
            .left_0()
            .right(px(panel))
            .bottom_0()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .px(px(18.0))
            .pt(px(22.0))
            .pb(px(10.0))
            // 由下往上淡出，而不是一条硬边：画面与控件的交界不该像被裁过。
            .bg(gpui_kit::linear_gradient(
                0.0,
                gpui_kit::linear_color_stop(gpui_kit::hsla(0.0, 0.0, 0.0, 0.92), 0.0),
                gpui_kit::linear_color_stop(gpui_kit::hsla(0.0, 0.0, 0.0, 0.0), 1.0),
            ))
            .on_mouse_move(cx.listener(|this, _: &MouseMoveEvent, _window, cx| {
                this.poke();
                cx.notify();
            }))
            // 控制条是一整条面板：点在按钮之间的空白处不应该穿透到画面去
            // 触发「单击暂停」。按钮自己已经 stop_propagation，这里兜住剩下的。
            .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                cx.stop_propagation();
            }))
            .child(self.render_progress(cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    // 左：播放控制
                    .child(icon_button(
                        "prev",
                        IconName::SkipBack,
                        false,
                        self.playlist().len() <= 1,
                        cx.listener(|this, _: &ClickEvent, _window, cx| this.previous_file(cx)),
                    ))
                    .child(icon_button(
                        "play",
                        if playing {
                            IconName::Pause
                        } else {
                            IconName::Play
                        },
                        false,
                        false,
                        cx.listener(|this, _: &ClickEvent, _window, cx| this.toggle_play(cx)),
                    ))
                    .child(icon_button(
                        "next",
                        IconName::SkipForward,
                        false,
                        self.playlist().len() <= 1,
                        cx.listener(|this, _: &ClickEvent, _window, cx| this.next_file(cx)),
                    ))
                    .child(div().w(px(6.0)))
                    .child(self.render_volume(cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .ml(px(8.0))
                            .text_size(px(12.0))
                            .text_color(text_muted())
                            .child(div().child(time_text)),
                    )
                    .child(div().flex_1())
                    // 右：功能开关
                    .child(pill_button(
                        "speed",
                        IconName::Gauge,
                        format_speed(snapshot.speed),
                        self.menu.is_some_and(Menu::is_speed)
                            || (snapshot.speed - 1.0).abs() > 0.001,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            // 预设列表和自定义面板是同一层，展开状态下再点一次直接收起。
                            this.menu = if this.menu.is_some_and(Menu::is_speed) {
                                None
                            } else {
                                Some(Menu::Speed)
                            };
                            this.menu_bar = None;
                            this.poke();
                            cx.notify();
                        }),
                    ))
                    .child(icon_button(
                        "subtitle",
                        IconName::Captions,
                        subtitle_active,
                        false,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.menu =
                                (this.menu != Some(Menu::Subtitle)).then_some(Menu::Subtitle);
                            this.menu_bar = None;
                            this.poke();
                            cx.notify();
                        }),
                    ))
                    .child(icon_button(
                        "audio",
                        IconName::AudioLines,
                        false,
                        !has_video,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.menu = (this.menu != Some(Menu::Audio)).then_some(Menu::Audio);
                            this.menu_bar = None;
                            this.poke();
                            cx.notify();
                        }),
                    ))
                    .child(pill_button(
                        "loop",
                        match self.settings.loop_mode {
                            LoopMode::Off | LoopMode::All => IconName::Repeat,
                            LoopMode::One => IconName::Repeat1,
                        },
                        self.settings.loop_mode.short_label(),
                        self.menu == Some(Menu::Loop) || self.settings.loop_mode != LoopMode::Off,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.menu = (this.menu != Some(Menu::Loop)).then_some(Menu::Loop);
                            this.menu_bar = None;
                            this.poke();
                            cx.notify();
                        }),
                    ))
                    .child(icon_button(
                        "playlist-toggle",
                        IconName::ListVideo,
                        self.settings.show_playlist,
                        false,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.set_playlist_visible(!this.settings.show_playlist);
                            this.poke();
                            cx.notify();
                        }),
                    ))
                    .child(icon_button(
                        "fullscreen",
                        IconName::Maximize,
                        false,
                        false,
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.toggle_fullscreen(window);
                            cx.notify();
                        }),
                    )),
            )
    }

    fn render_menu(&self, panel: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu?;
        // 自定义倍速是一块独立面板（标题栏 + 滑块），不套列表布局。
        if menu == Menu::SpeedCustom {
            return Some(self.render_speed_panel(panel, cx));
        }
        let mut entries: Vec<(String, bool, Option<MenuAction>)> = Vec::new();

        match menu {
            Menu::Speed => {
                for speed in SPEED_PRESETS {
                    entries.push((
                        format_speed(speed),
                        (self.snapshot.speed - speed).abs() < 0.001,
                        Some(MenuAction::Command(Command::SetSpeed(speed))),
                    ));
                }
                // 用键盘调出来的档位（如 1.15×）不在预设里，直接在这一行标出来，
                // 否则打开菜单看到的是一排都没选中的数字。
                let custom = !SPEED_PRESETS
                    .iter()
                    .any(|preset| (self.snapshot.speed - preset).abs() < 0.001);
                entries.push((
                    if custom {
                        format!("自定义倍数 · {}", format_speed_value(self.snapshot.speed))
                    } else {
                        "自定义倍数…".to_string()
                    },
                    custom,
                    Some(MenuAction::SpeedCustom),
                ));
            }
            // 上面已经单独返回，这里不需要内容。
            Menu::SpeedCustom => {}
            Menu::Loop => {
                for mode in [LoopMode::Off, LoopMode::All, LoopMode::One] {
                    entries.push((
                        mode.label().to_string(),
                        self.settings.loop_mode == mode,
                        Some(MenuAction::Loop(mode)),
                    ));
                }
            }
            Menu::Subtitle => {
                entries.push((
                    "关闭字幕".to_string(),
                    self.snapshot
                        .selected_track(crate::player::TrackKind::Subtitle)
                        .is_none(),
                    Some(MenuAction::Command(Command::SelectTrack {
                        kind: crate::player::TrackKind::Subtitle,
                        id: -1,
                    })),
                ));
                for track in self.snapshot.track(crate::player::TrackKind::Subtitle) {
                    entries.push((
                        track.display_name(),
                        track.selected,
                        Some(MenuAction::Command(Command::SelectTrack {
                            kind: crate::player::TrackKind::Subtitle,
                            id: track.id,
                        })),
                    ));
                }
            }
            Menu::Audio => {
                for track in self.snapshot.track(crate::player::TrackKind::Audio) {
                    entries.push((
                        track.display_name(),
                        track.selected,
                        Some(MenuAction::Command(Command::SelectTrack {
                            kind: crate::player::TrackKind::Audio,
                            id: track.id,
                        })),
                    ));
                }
                if entries.is_empty() {
                    entries.push(("没有可切换的音轨".to_string(), false, None));
                }
            }
        }

        Some(
            div()
                .id("menu")
                .absolute()
                // 跟着控制条一起让开面板，否则弹出菜单会飘到侧边栏上。
                .right(px(18.0 + panel))
                .bottom(px(74.0))
                .w(px(220.0))
                .py(px(6.0))
                .rounded(px(10.0))
                .bg(surface())
                .border_1()
                .border_color(gpui_kit::rgb(0x2c2c34))
                .shadow_lg()
                // 菜单项自己不管冒泡：这一层统一把点击吃掉，否则选个倍速/字幕
                // 会连带把点击漏给画面，顺带切一次播放状态。
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .children(entries.into_iter().enumerate().map(
                    |(index, (label, active, action))| {
                        let id = gpui_kit::ElementId::Name(format!("menu-{index}").into());
                        let enabled = action.is_some();
                        div()
                            .id(id)
                            .px(px(10.0))
                            .py(px(7.0))
                            .mx(px(4.0))
                            .rounded(px(6.0))
                            .text_size(px(13.0))
                            .text_color(if active {
                                ui::accent()
                            } else if enabled {
                                text()
                            } else {
                                text_faint()
                            })
                            .when(enabled, |this| {
                                this.cursor(CursorStyle::PointingHand)
                                    .hover(|style| style.bg(ui::surface_hover()))
                            })
                            .child(label)
                            .when_some(action, |this, action| {
                                this.on_click(cx.listener(
                                    move |this, _: &ClickEvent, _window, cx| {
                                        match action.clone() {
                                            MenuAction::Command(command) => {
                                                this.send(command);
                                                this.menu = None;
                                            }
                                            MenuAction::Loop(mode) => {
                                                this.set_loop(mode, cx);
                                                this.menu = None;
                                            }
                                            // 自定义面板是倍速菜单的第二层，不关菜单只换内容。
                                            MenuAction::SpeedCustom => {
                                                this.menu = Some(Menu::SpeedCustom);
                                            }
                                        }
                                        this.poke();
                                        cx.notify();
                                    },
                                ))
                            })
                    },
                ))
                // 字幕菜单额外提供一个「从文件加载」入口。
                .when(menu == Menu::Subtitle, |this| {
                    this.child(
                        div()
                            .h(px(1.0))
                            .mx(px(8.0))
                            .my(px(4.0))
                            .bg(gpui_kit::rgb(0x2c2c34)),
                    )
                    .child(
                        div()
                            .id("menu-load-sub")
                            .px(px(10.0))
                            .py(px(7.0))
                            .mx(px(4.0))
                            .rounded(px(6.0))
                            .text_size(px(13.0))
                            .text_color(text_muted())
                            .cursor(CursorStyle::PointingHand)
                            .hover(|style| style.bg(ui::surface_hover()))
                            .child("从文件加载字幕…")
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.menu = None;
                                this.pick_subtitle(window, cx);
                            })),
                    )
                })
                .into_any_element(),
        )
    }

    /// 自定义倍速面板：标题栏 + 跟随滑块的气泡 + 0.1–5.0 的滑杆。
    ///
    /// 气泡和滑块头的位置都用固定宽度算，而不是读 canvas 回填的 bounds：bounds
    /// 要到 prepaint 之后才有值，拿它排版会让气泡慢一帧。canvas 那份只负责把
    /// 真实命中区域写回 `speed_bounds`，供拖动时换算坐标。
    fn render_speed_panel(&self, panel: f32, cx: &mut Context<Self>) -> AnyElement {
        const WIDTH: f32 = 264.0;
        const PAD: f32 = 14.0;
        const LABEL: f32 = 22.0;
        const GAP: f32 = 8.0;
        const BUBBLE: f32 = 56.0;
        const TRACK: f32 = WIDTH - PAD * 2.0 - (LABEL + GAP) * 2.0;

        let speed = self.snapshot.speed.clamp(SPEED_MIN, SPEED_MAX);
        let ratio = ((speed - SPEED_MIN) / (SPEED_MAX - SPEED_MIN)).clamp(0.0, 1.0) as f32;
        let dragging = self.speed_dragging;
        // 气泡中心跟着滑块头走，靠近两端时收在面板内。
        let bubble_left =
            (PAD + LABEL + GAP + ratio * TRACK - BUBBLE / 2.0).clamp(PAD, WIDTH - PAD - BUBBLE);
        let knob = if dragging { 14.0 } else { 12.0 };
        let slot = self.speed_bounds.clone();

        div()
            .id("speed-menu")
            .absolute()
            .right(px(18.0 + panel))
            .bottom(px(74.0))
            .w(px(WIDTH))
            .py(px(6.0))
            .rounded(px(10.0))
            .bg(surface())
            .border_1()
            .border_color(gpui_kit::rgb(0x2c2c34))
            .shadow_lg()
            // 和列表菜单一样挡住点击，否则点面板空白处会穿透到画面切一次播放。
            .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                cx.stop_propagation();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(10.0))
                    .py(px(4.0))
                    .child(
                        div()
                            .id("speed-back")
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(24.0))
                            .rounded(px(6.0))
                            .cursor(CursorStyle::PointingHand)
                            .hover(|style| style.bg(ui::surface_hover()))
                            .child(icon(IconName::ChevronLeft, 16.0, text()))
                            .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                                this.menu = Some(Menu::Speed);
                                this.poke();
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(text())
                            .child("自定义倍数"),
                    ),
            )
            .child(
                div()
                    .h(px(1.0))
                    .mx(px(10.0))
                    .my(px(4.0))
                    .bg(gpui_kit::rgb(0x2c2c34)),
            )
            .child(
                div().relative().h(px(30.0)).w_full().child(
                    div()
                        .absolute()
                        .left(px(bubble_left))
                        .top(px(2.0))
                        .w(px(BUBBLE))
                        .h(px(24.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.0))
                        .bg(ui::surface_hover())
                        .text_size(px(12.0))
                        .text_color(text())
                        .child(format_speed_value(speed)),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(GAP))
                    .px(px(PAD))
                    .pb(px(8.0))
                    .child(speed_edge_label("0.1", LABEL))
                    .child(
                        div()
                            .id("speed-slider")
                            .relative()
                            .flex_1()
                            .h(px(20.0))
                            .flex()
                            .items_center()
                            .cursor(CursorStyle::PointingHand)
                            .child(
                                canvas(
                                    move |bounds, _window, _cx| slot.set(bounds),
                                    |_bounds, (), _window, _cx| {},
                                )
                                .absolute()
                                .inset_0(),
                            )
                            .child(
                                div()
                                    .relative()
                                    .h(px(3.0))
                                    .w_full()
                                    .rounded(px(2.0))
                                    .bg(ui::track())
                                    .child(
                                        div()
                                            .absolute()
                                            .left_0()
                                            .top_0()
                                            .bottom_0()
                                            .w(gpui_kit::relative(ratio))
                                            .rounded(px(2.0))
                                            .bg(text())
                                            .child(
                                                div()
                                                    .absolute()
                                                    .right(px(-knob / 2.0))
                                                    .top(px(-(knob - 3.0) / 2.0))
                                                    .size(px(knob))
                                                    .rounded_full()
                                                    .bg(text()),
                                            ),
                                    ),
                            )
                            // 刻度只标整数档：全量程铺满反而看不清滑块在哪一档。
                            .children((1..=4).map(|tick| {
                                let at = (f64::from(tick) - SPEED_MIN) / (SPEED_MAX - SPEED_MIN);
                                div()
                                    .absolute()
                                    .left(gpui_kit::relative(at as f32))
                                    .top(px(8.0))
                                    .w(px(1.0))
                                    .h(px(4.0))
                                    .bg(text_faint())
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                    this.speed_dragging = true;
                                    this.set_speed_from_x(event.position.x, cx);
                                    // 按下时即使数值没变也要重画：滑块头要放大一圈。
                                    cx.notify();
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseUpEvent, _window, cx| {
                                    this.speed_dragging = false;
                                    // 松手才落盘：拖动途中每移动一像素写一次太浪费。
                                    this.settings.save();
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(speed_edge_label("5.0", LABEL)),
            )
            .into_any_element()
    }

    fn bar_entries(&self, menu: BarMenu, window: &Window) -> Vec<BarItem> {
        use BarAction::*;

        let mut items: Vec<BarItem> = Vec::new();
        match menu {
            BarMenu::File => {
                items.push(BarItem::Entry(
                    BarEntry::new("打开文件…", OpenFile).hint("O"),
                ));
                items.push(BarItem::Entry(BarEntry::new("打开文件夹…", OpenFolder)));
                items.push(BarItem::Separator);
                items.push(BarItem::Entry(BarEntry::new("退出", Quit)));
            }
            BarMenu::Playback => {
                items.push(BarItem::Entry(
                    BarEntry::new(
                        if self.snapshot.paused {
                            "播放"
                        } else {
                            "暂停"
                        },
                        Command(crate::player::Command::PlayPause),
                    )
                    .hint("空格"),
                ));
                items.push(BarItem::Entry(BarEntry::new(
                    "停止",
                    Command(crate::player::Command::Stop),
                )));
                items.push(BarItem::Separator);

                let navigable = self.playlist().len() > 1;
                items.push(BarItem::Entry(
                    BarEntry::new("上一集", Previous)
                        .hint("N")
                        .enabled(navigable),
                ));
                items.push(BarItem::Entry(
                    BarEntry::new("下一集", Next).hint("P").enabled(navigable),
                ));
                items.push(BarItem::Separator);
                items.push(BarItem::Entry(
                    BarEntry::new("快退 5 秒", Seek(-SEEK_STEP)).hint("←"),
                ));
                items.push(BarItem::Entry(
                    BarEntry::new("快进 5 秒", Seek(SEEK_STEP)).hint("→"),
                ));
                items.push(BarItem::Separator);

                for mode in [LoopMode::Off, LoopMode::All, LoopMode::One] {
                    items.push(BarItem::Entry(
                        BarEntry::new(mode.label(), SetLoop(mode))
                            .checked(self.settings.loop_mode == mode),
                    ));
                }

                items.push(BarItem::Separator);
                items.push(BarItem::Entry(
                    BarEntry::new("截图", TakeScreenshot)
                        .hint("S")
                        .enabled(self.snapshot.has_video()),
                ));
            }
            BarMenu::Subtitle => {
                items.push(BarItem::Entry(
                    BarEntry::new(
                        "关闭字幕",
                        Command(crate::player::Command::SelectTrack {
                            kind: TrackKind::Subtitle,
                            id: -1,
                        }),
                    )
                    .checked(self.snapshot.selected_track(TrackKind::Subtitle).is_none()),
                ));

                let tracks = self.snapshot.track(TrackKind::Subtitle);
                if !tracks.is_empty() {
                    items.push(BarItem::Separator);
                }
                for track in tracks {
                    items.push(BarItem::Entry(
                        BarEntry::new(
                            track.display_name(),
                            Command(crate::player::Command::SelectTrack {
                                kind: TrackKind::Subtitle,
                                id: track.id,
                            }),
                        )
                        .checked(track.selected),
                    ));
                }

                items.push(BarItem::Separator);
                items.push(BarItem::Entry(BarEntry::new(
                    "从文件加载字幕…",
                    LoadSubtitle,
                )));
            }
            BarMenu::Audio => {
                let tracks = self.snapshot.track(TrackKind::Audio);
                if tracks.is_empty() {
                    items.push(BarItem::Entry(BarEntry::note("没有可切换的音轨")));
                }
                for track in tracks {
                    items.push(BarItem::Entry(
                        BarEntry::new(
                            track.display_name(),
                            Command(crate::player::Command::SelectTrack {
                                kind: TrackKind::Audio,
                                id: track.id,
                            }),
                        )
                        .checked(track.selected),
                    ));
                }
            }
            BarMenu::View => {
                items.push(BarItem::Entry(
                    BarEntry::new("播放列表", TogglePlaylist)
                        .hint("L")
                        .checked(self.settings.show_playlist),
                ));
                items.push(BarItem::Entry(
                    BarEntry::new("全屏", ToggleFullscreen)
                        .hint("F")
                        .checked(window.is_fullscreen()),
                ));
            }
            BarMenu::Settings => {
                let supported = platform::associations::supported();
                let status = &self.association_status;

                items.push(BarItem::Entry(
                    BarEntry::new("设为默认视频播放器…", SetAsDefaultPlayer).enabled(supported),
                ));
                if cfg!(windows) {
                    items.push(BarItem::Entry(
                        BarEntry::new("打开系统默认应用设置", OpenDefaultApps).enabled(supported),
                    ));
                }
                items.push(BarItem::Separator);
                items.push(BarItem::Entry(
                    BarEntry::new("移除本程序的文件关联", RemoveAssociations)
                        .enabled(status.is_registered()),
                ));
                items.push(BarItem::Separator);
                items.push(BarItem::Entry(
                    BarEntry::new("启动时自动维护关联", ToggleMaintainAssociations)
                        .checked(self.settings.maintain_associations)
                        .enabled(supported),
                ));
                items.push(BarItem::Separator);
                items.push(BarItem::Entry(BarEntry::note(association_status_line(
                    status, supported,
                ))));
            }
            BarMenu::Help => {
                items.push(BarItem::Entry(
                    BarEntry::new("键盘快捷键", ToggleHelp)
                        .hint("?")
                        .checked(self.show_help),
                ));
                items.push(BarItem::Separator);
                items.push(BarItem::Entry(BarEntry::new(
                    "关于 ngy-video-player",
                    About,
                )));
            }
        }

        items
    }

    fn run_bar_action(&mut self, action: BarAction, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_bar = None;
        match action {
            BarAction::Command(command) => self.send(command),
            BarAction::OpenFile => self.pick_file(window, cx),
            BarAction::OpenFolder => self.pick_folder(window, cx),
            BarAction::LoadSubtitle => self.pick_subtitle(window, cx),
            BarAction::Previous => self.previous_file(cx),
            BarAction::Next => self.next_file(cx),
            BarAction::Seek(delta) => self.seek_by(delta, cx),
            BarAction::SetLoop(mode) => self.set_loop(mode, cx),
            BarAction::TogglePlaylist => {
                self.set_playlist_visible(!self.settings.show_playlist);
            }
            BarAction::ToggleFullscreen => self.toggle_fullscreen(window),
            BarAction::ToggleHelp => self.show_help = !self.show_help,
            BarAction::TakeScreenshot => self.take_screenshot(cx),
            BarAction::SetAsDefaultPlayer => self.set_default_player(),
            BarAction::OpenDefaultApps => self.open_default_apps(),
            BarAction::RemoveAssociations => self.remove_associations(window, cx),
            BarAction::ToggleMaintainAssociations => self.toggle_maintain_associations(),
            BarAction::About => {
                self.flash(format!(
                    "ngy-video-player {}\nGPUI + libmpv",
                    env!("CARGO_PKG_VERSION")
                ));
            }
            BarAction::Quit => {
                cx.quit();
                return;
            }
        }
        self.poke();
        cx.notify();
    }

    // -- 文件关联 ----------------------------------------------------------

    /// 用户主动点了「设为默认」，所以成败都要说清楚——和启动时的静默自愈不同。
    fn set_default_player(&mut self) {
        if !platform::associations::supported() {
            self.flash(format!(
                "{} 上还没有实现自动关联，请手动在系统设置里指定默认播放器",
                std::env::consts::OS
            ));
            return;
        }

        match platform::associations::register() {
            Ok(status) => {
                self.refresh_association_status();
                let mut lines = Vec::new();
                if let Ok(exe) = std::env::current_exe()
                    && is_development_build(&exe)
                {
                    lines.push(format!("注意：这是开发构建，关联指向 {}", exe.display()));
                    lines.push("cargo clean 之后关联会失效。".to_string());
                }
                lines.push(format!("已登记 {} 个格式。", status.registered));

                // Windows 上还得用户去设置页点一下；另两个平台注册即生效。
                match platform::associations::open_system_settings() {
                    Ok(()) => lines.push(platform::associations::guidance().to_string()),
                    Err(_) => lines.push("请打开 设置 > 应用 > 默认应用 选择本程序。".to_string()),
                }
                if !self.settings.maintain_associations {
                    lines.push("自动维护已关闭，本次只注册一次。".to_string());
                }
                self.flash(lines.join("\n"));
            }
            Err(error) => self.report_association_failure("注册文件关联失败", &error),
        }
    }

    fn open_default_apps(&mut self) {
        match platform::associations::open_system_settings() {
            Ok(()) => self.flash("已打开系统的「默认应用」设置"),
            Err(error) => self.report_association_failure("打开系统设置失败", &error),
        }
    }

    fn toggle_maintain_associations(&mut self) {
        self.settings.maintain_associations = !self.settings.maintain_associations;
        // 这个开关决定的是「下次启动」的行为，等到窗口关闭再落盘可能就等不到了。
        self.settings.save();
        if self.settings.maintain_associations {
            self.flash("已开启：每次启动重新登记文件关联");
        } else {
            self.flash("已关闭：启动时不再改动文件关联");
        }
    }

    fn remove_associations(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let description = format!(
            "将删除本程序向系统登记的全部文件关联（{} 个格式）。\n\n\
             系统里已经选定的默认播放器不受影响——要改回来，请到 设置 > 应用 > 默认应用 手动调整。",
            platform::associations::count()
        );
        let dialog = rfd::MessageDialog::new()
            .set_parent(window)
            .set_title("移除文件关联")
            .set_description(description)
            .set_buttons(rfd::MessageButtons::YesNo);

        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(dialog.show() == rfd::MessageDialogResult::Yes);
        });

        cx.spawn(async move |this, cx| {
            let confirmed = rx.await.unwrap_or(false);
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if confirmed {
                    match platform::associations::remove() {
                        Ok(()) => {
                            this.refresh_association_status();
                            this.flash("已移除本程序的文件关联");
                        }
                        Err(error) => this.report_association_failure("移除文件关联失败", &error),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 自动维护：把注册项按当前 exe 的路径重写一遍。
    ///
    /// 只在开关打开、平台支持、并且注册项确实过期时才动手——已经指对了就什么都不做。
    /// 放后台线程：启动路径上不该花时间等注册表。
    fn auto_maintain_associations(&self) {
        if !self.settings.maintain_associations || !platform::associations::supported() {
            return;
        }
        std::thread::spawn(|| {
            let status = platform::associations::status();
            let up_to_date = status.registered == platform::associations::count()
                && status
                    .target
                    .as_ref()
                    .is_some_and(|target| association_target_matches(target));
            if up_to_date {
                return;
            }
            // 启动时的失败不打扰用户：注册表被策略锁死时每次开机糊一脸错更烦人。
            if let Err(error) = platform::associations::register() {
                log_warn!("自动维护文件关联失败: {error:#}");
            }
        });
    }

    fn refresh_association_status(&mut self) {
        self.association_status = platform::associations::status();
    }

    /// 关联失败统一走红色横幅：里面的手动操作路径一行字放不下。
    fn report_association_failure(&mut self, title: &'static str, error: &anyhow::Error) {
        log_warn!("{title}: {error:#}");
        self.flash(title);
        self.error = Some(Banner::new(
            title,
            format!("{error:#}\n\n也可以在系统设置里手动指定默认播放器。"),
        ));
    }

    fn render_menu_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("menu-bar")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(MENUBAR_HEIGHT))
            .flex()
            .items_center()
            .px(px(8.0))
            .bg(surface())
            .border_b_1()
            .border_color(gpui_kit::rgb(0x26262e))
            .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                cx.stop_propagation();
            }))
            .children(
                BarMenu::ALL
                    .iter()
                    .map(|menu| self.render_bar_menu(*menu, window, cx).into_any_element()),
            )
    }

    fn render_bar_menu(
        &self,
        menu: BarMenu,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.menu_bar == Some(menu);
        div()
            .relative()
            .child(
                div()
                    .id(gpui_kit::ElementId::Name(
                        format!("bar-menu-{menu:?}").into(),
                    ))
                    .h(px(26.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .rounded(px(6.0))
                    .text_size(px(13.0))
                    .cursor(CursorStyle::PointingHand)
                    .text_color(if open { ui::accent() } else { text_muted() })
                    .when(open, |this| this.bg(ui::surface_hover()))
                    .when(!open, |this| {
                        this.hover(|style| style.bg(ui::surface_hover()))
                    })
                    .child(menu.label())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.menu_bar = (this.menu_bar != Some(menu)).then_some(menu);
                        if this.menu_bar.is_some() {
                            this.menu = None;
                        }
                        if this.menu_bar == Some(BarMenu::Settings) {
                            this.refresh_association_status();
                        }
                        this.poke();
                        cx.notify();
                    }))
                    // 已经展开一个菜单时，移过其它标题直接切换，符合桌面菜单习惯。
                    .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                        if *hovered && this.menu_bar.is_some() && this.menu_bar != Some(menu) {
                            this.menu_bar = Some(menu);
                            if menu == BarMenu::Settings {
                                this.refresh_association_status();
                            }
                            this.poke();
                            cx.notify();
                        }
                    })),
            )
            .when(open, |this| {
                this.child(self.render_bar_dropdown(menu, window, cx))
            })
    }

    fn render_bar_dropdown(
        &self,
        menu: BarMenu,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let available = f32::from(window.viewport_size().height) - MENUBAR_HEIGHT - 16.0;
        div()
            .id(gpui_kit::ElementId::Name(
                format!("bar-dropdown-{menu:?}").into(),
            ))
            .absolute()
            .left(px(-4.0))
            .top(px(28.0))
            .w(px(250.0))
            .max_h(px(available.clamp(160.0, 420.0)))
            .overflow_y_scroll()
            .py(px(5.0))
            .rounded(px(10.0))
            .bg(surface())
            .border_1()
            .border_color(gpui_kit::rgb(0x2c2c34))
            .shadow_lg()
            .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                cx.stop_propagation();
            }))
            .children(self.bar_entries(menu, window).into_iter().enumerate().map(
                |(index, item)| {
                    match item {
                        BarItem::Separator => div()
                            .h(px(1.0))
                            .mx(px(8.0))
                            .my(px(4.0))
                            .bg(gpui_kit::rgb(0x2c2c34))
                            .into_any_element(),
                        BarItem::Entry(entry) => self
                            .render_bar_entry(menu, index, entry, cx)
                            .into_any_element(),
                    }
                },
            ))
    }

    fn render_bar_entry(
        &self,
        menu: BarMenu,
        index: usize,
        entry: BarEntry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let BarEntry {
            label,
            hint,
            active,
            enabled,
            action,
        } = entry;
        let action = action.filter(|_| enabled);
        let color = if !enabled {
            text_faint()
        } else if active {
            ui::accent()
        } else {
            text()
        };

        div()
            .id(gpui_kit::ElementId::Name(
                format!("bar-item-{menu:?}-{index}").into(),
            ))
            .flex()
            .items_center()
            .gap(px(8.0))
            .h(px(30.0))
            .mx(px(4.0))
            .px(px(10.0))
            .rounded(px(6.0))
            .text_size(px(13.0))
            .text_color(color)
            .when(enabled, |this| {
                this.cursor(CursorStyle::PointingHand)
                    .hover(|style| style.bg(ui::surface_hover()))
            })
            .child(
                div()
                    .w(px(14.0))
                    .flex()
                    .justify_center()
                    .when(active, |this| {
                        this.child(icon(IconName::Check, 14.0, ui::accent()))
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(label),
            )
            .when_some(hint, |this, hint| {
                this.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(text_faint())
                        .child(hint),
                )
            })
            .when_some(action, |this, action| {
                this.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.run_bar_action(action.clone(), window, cx);
                }))
            })
    }

    /// 面板顶部的页签条：多份列表并排，`+` 新建，`×` 关闭（只剩一页时不显示）。
    fn render_playlist_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.playlists.active();
        let closable = self.playlists.len() > 1;

        div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .px(px(8.0))
            .pb(px(6.0))
            .child(
                div()
                    .id("playlist-tabs")
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_x_scroll()
                    .children(
                        self.playlists
                            .tabs()
                            .iter()
                            .enumerate()
                            .map(|(index, tab)| {
                                let group = format!("playlist-tab-{index}");
                                let is_active = index == active;
                                div()
                                    .id(gpui_kit::ElementId::Name(
                                        format!("playlist-tab-{index}").into(),
                                    ))
                                    .group(group.clone())
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .h(px(24.0))
                                    .px(px(8.0))
                                    .rounded(px(6.0))
                                    .text_size(px(12.0))
                                    .cursor(CursorStyle::PointingHand)
                                    .text_color(if is_active {
                                        ui::accent()
                                    } else {
                                        text_muted()
                                    })
                                    .when(is_active, |this| this.bg(gpui_kit::rgb(0x232330)))
                                    .when(!is_active, |this| {
                                        this.hover(|style| style.bg(ui::surface_hover()))
                                    })
                                    // 切页不打断播放，所以「正在出声的是哪一页」得单独标出来。
                                    .when(self.playing_tab == Some(index), |this| {
                                        this.child(icon(IconName::Play, 8.0, ui::accent()))
                                    })
                                    .child(
                                        div()
                                            .max_w(px(140.0))
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(tab.name.clone()),
                                    )
                                    .when(closable, |this| {
                                        this.child(
                                            div()
                                                .id(gpui_kit::ElementId::Name(
                                                    format!("playlist-tab-close-{index}").into(),
                                                ))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .size(px(14.0))
                                                .rounded(px(4.0))
                                                // 和行尾的删除按钮一样：指到这一页才露出来。
                                                .opacity(0.0)
                                                .group_hover(group, |style| style.opacity(1.0))
                                                .hover(|style| style.bg(ui::surface_active()))
                                                .child(icon(IconName::X, 10.0, text_faint()))
                                                .on_click(cx.listener(
                                                    move |this, _: &ClickEvent, _window, cx| {
                                                        // 不截断的话这一下还会落到页签的「切换」上。
                                                        cx.stop_propagation();
                                                        this.close_tab(index, cx);
                                                    },
                                                )),
                                        )
                                    })
                                    .on_click(cx.listener(
                                        move |this, _: &ClickEvent, _window, cx| {
                                            this.select_tab(index, cx);
                                        },
                                    ))
                            }),
                    ),
            )
            .child(
                div()
                    .id("playlist-add-tab")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(22.0))
                    .rounded(px(6.0))
                    .text_size(px(14.0))
                    .text_color(text_muted())
                    .cursor(CursorStyle::PointingHand)
                    .hover(|style| style.bg(ui::surface_hover()).text_color(text()))
                    .child("+")
                    .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| this.add_tab(cx))),
            )
    }

    fn render_playlist(&self, top: Pixels, width: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.playlist().index();
        let len = self.playlist().len();
        let selected = self.selection.len();
        let multi_hint = if cfg!(target_os = "macos") {
            "⌘ 点击多选 · Shift 连选 · Delete 删除"
        } else {
            "Ctrl 点击多选 · Shift 连选 · Delete 删除"
        };

        div()
            .id("playlist")
            .absolute()
            .top(top)
            .right_0()
            .bottom_0()
            .w(px(width))
            .flex()
            .flex_col()
            .bg(gpui_kit::rgba(0x131317e6))
            .border_l_1()
            .border_color(gpui_kit::rgb(0x26262e))
            // 面板上的点击（选条目、点关闭）不该漏到画面去。
            .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                cx.stop_propagation();
                if this.menu_bar.is_some() {
                    this.menu_bar = None;
                    cx.notify();
                }
            }))
            // 跨在面板左边界上的拖动条：往左拖更宽，往右拖更窄。
            .child(
                div()
                    .id("playlist-resizer")
                    .absolute()
                    .left(px(-PLAYLIST_RESIZER_WIDTH / 2.0))
                    .top_0()
                    .bottom_0()
                    .w(px(PLAYLIST_RESIZER_WIDTH))
                    .cursor(CursorStyle::ResizeLeftRight)
                    .when(self.playlist_dragging, |this| this.bg(ui::accent()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.playlist_dragging = true;
                            this.drag_playlist_width(event.position.x, window);
                            cx.notify();
                        }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pl(px(14.0))
                    .pr(px(6.0))
                    .py(px(8.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(text_muted())
                            .child(format!("播放列表 · {len}")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.0))
                            // 用文字而不是垃圾桶图标：标题栏的图标离「关闭」太近，
                            // 而且和行尾的「移除这一项」撞脸，一不留神就清空了整张表。
                            .when(len > 0, |this| {
                                this.child(
                                    div()
                                        .id("playlist-clear")
                                        .px(px(6.0))
                                        .py(px(3.0))
                                        .rounded(px(6.0))
                                        .text_size(px(12.0))
                                        .text_color(text_muted())
                                        .cursor(CursorStyle::PointingHand)
                                        .hover(|style| {
                                            style.bg(ui::surface_hover()).text_color(ui::danger())
                                        })
                                        .child("清空")
                                        .on_click(cx.listener(
                                            |this, _: &ClickEvent, _window, cx| {
                                                cx.stop_propagation();
                                                this.clear_playlist(cx);
                                            },
                                        )),
                                )
                            })
                            .child(icon_button(
                                "playlist-close",
                                IconName::X,
                                false,
                                false,
                                cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    this.set_playlist_visible(false);
                                    cx.notify();
                                }),
                            )),
                    ),
            )
            .child(self.render_playlist_tabs(cx))
            // 有选择时才出现的一小条：把「删多少、删哪些」摆在手上。
            .when(selected > 0, |this| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(14.0))
                        .pb(px(8.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(ui::accent())
                                .child(format!("已选 {selected} 项")),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .id("playlist-remove-selected")
                                        .px(px(6.0))
                                        .py(px(3.0))
                                        .rounded(px(6.0))
                                        .text_size(px(12.0))
                                        .text_color(ui::danger())
                                        .cursor(CursorStyle::PointingHand)
                                        .hover(|style| style.bg(ui::surface_hover()))
                                        .child("删除所选")
                                        .on_click(cx.listener(
                                            |this, _: &ClickEvent, _window, cx| {
                                                cx.stop_propagation();
                                                this.remove_selection(cx);
                                            },
                                        )),
                                )
                                .child(
                                    div()
                                        .id("playlist-clear-selection")
                                        .px(px(6.0))
                                        .py(px(3.0))
                                        .rounded(px(6.0))
                                        .text_size(px(12.0))
                                        .text_color(text_muted())
                                        .cursor(CursorStyle::PointingHand)
                                        .hover(|style| style.bg(ui::surface_hover()))
                                        .child("取消")
                                        .on_click(cx.listener(
                                            |this, _: &ClickEvent, _window, cx| {
                                                cx.stop_propagation();
                                                this.selection.clear();
                                                this.selection_anchor = None;
                                                cx.notify();
                                            },
                                        )),
                                ),
                        ),
                )
            })
            .when(len == 0, |this| {
                this.child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(8.0))
                        .child(icon(IconName::Film, 28.0, text_faint()))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(text_faint())
                                .child("列表为空"),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(text_faint())
                                .child("拖入文件，或按 O 打开"),
                        ),
                )
            })
            .when(len > 0, |this| {
                this.child(
                    div()
                        .id("playlist-scroll")
                        .flex_1()
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .children(self.playlist().items().iter().enumerate().map(
                            |(index, path)| self.render_playlist_row(index, path, current, cx),
                        )),
                )
                .child(
                    div()
                        .px(px(14.0))
                        .py(px(8.0))
                        .text_size(px(11.0))
                        .text_color(text_faint())
                        .child(multi_hint),
                )
            })
    }

    fn render_playlist_row(
        &self,
        index: usize,
        path: &std::path::Path,
        current: Option<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = path
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        let is_current = current == Some(index);
        let is_selected = self.selection.contains(&index);
        let group = format!("playlist-row-{index}");
        let row_path = path.to_path_buf();

        div()
            .id(gpui_kit::ElementId::Name(format!("item-{index}").into()))
            // 行尾的删除按钮只在这一行被指向时出现，靠 group hover 的样式下发；
            // 不用界面状态另存一个 hovered 索引，省掉每次进出都整窗重绘。
            .group(group.clone())
            .flex()
            .items_center()
            .gap(px(8.0))
            .pl(px(10.0))
            .pr(px(6.0))
            .py(px(8.0))
            .when(is_current, |this| this.bg(gpui_kit::rgb(0x1f2a3a)))
            .when(is_selected && !is_current, |this| {
                this.bg(gpui_kit::rgba(0x5aa9ff1f))
            })
            .when(!is_current, |this| {
                this.hover(|style| style.bg(ui::surface_hover()))
            })
            .cursor(CursorStyle::PointingHand)
            .child(
                // 勾选标记占固定一格，选中与未选中之间文字不会左右跳。
                div()
                    .w(px(14.0))
                    .flex()
                    .justify_center()
                    .when(is_selected, |this| {
                        this.child(icon(IconName::Check, 12.0, ui::accent()))
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(if is_current { ui::accent() } else { text() })
                            .child(ui::ellipsis(name)),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text_faint())
                            .child(formats::kind_label(path).to_string()),
                    ),
            )
            .child(
                div()
                    .id(gpui_kit::ElementId::Name(
                        format!("item-remove-{index}").into(),
                    ))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(24.0))
                    .rounded(px(6.0))
                    .cursor(CursorStyle::PointingHand)
                    // 占位与常显时一致，鼠标移上去才淡入，文字不会跟着左右跳。
                    .opacity(0.0)
                    .group_hover(group, |style| style.opacity(1.0))
                    .hover(|style| style.bg(ui::surface_hover()))
                    .child(icon(IconName::Trash, 14.0, text_faint()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        // 不截断的话，同一次点击还会落到整行的「换歌」上。
                        cx.stop_propagation();
                        this.remove_items(&[index], cx);
                    })),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
                this.select_item(index, event.modifiers(), cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    // 右键只弹菜单：不选中、不换歌，也不让点击漏到画面上。
                    cx.stop_propagation();
                    this.open_item_menu(index, row_path.clone(), event.position, window, cx);
                }),
            )
            .into_any_element()
    }

    fn render_osd(&self, top: Pixels, panel: f32) -> Option<impl IntoElement> {
        let (lines, _) = self.osd.as_ref()?;
        Some(
            div()
                .absolute()
                .left(px(0.0))
                // 提示条在画面上居中，别压到侧边栏上。
                .right(px(panel))
                .top(top)
                .flex()
                .justify_center()
                .child(
                    div()
                        .px(px(14.0))
                        .py(px(8.0))
                        .rounded(px(10.0))
                        .bg(gpui_kit::rgba(0x000000cc))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(2.0))
                        .children(lines.iter().map(|line| {
                            div()
                                .text_size(px(14.0))
                                .text_color(text())
                                .child(line.clone())
                        })),
                ),
        )
    }

    /// 截图卡片：预览 + 复制 / 打开 / 定位。
    ///
    /// 放左上角：画面底部是字幕、控件也在那儿，卡片压过去会挡住用户真正在看的东西。
    fn render_screenshot_card(
        &self,
        top: Pixels,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let card = self.screenshot_card.as_ref()?;

        let preview = match &card.thumb {
            ScreenshotThumb::Ready(thumb) => img(thumb.clone()).size_full().into_any_element(),
            ScreenshotThumb::Loading => placeholder("正在读取预览…"),
            ScreenshotThumb::Failed => placeholder("预览不可用"),
        };

        Some(
            div()
                .id("screenshot-card")
                .absolute()
                .left(px(24.0))
                .top(top)
                .w(px(SCREENSHOT_CARD_WIDTH))
                .rounded(px(12.0))
                .bg(surface())
                .border_1()
                .border_color(gpui_kit::rgb(0x2c2c34))
                .shadow_lg()
                .overflow_hidden()
                // 卡片浮在画面上：点击和按下都在这里吃掉，否则会漏到底下触发暂停。
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_this, _: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                    }),
                )
                .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                    if let Some(card) = this.screenshot_card.as_mut() {
                        card.hovered = *hovered;
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(SCREENSHOT_PREVIEW_HEIGHT))
                        .bg(gpui_kit::rgb(0x08080a))
                        .child(preview)
                        .child(
                            div()
                                .absolute()
                                .top(px(6.0))
                                .right(px(6.0))
                                .child(icon_button(
                                    "screenshot-card-close",
                                    IconName::X,
                                    false,
                                    false,
                                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                                        this.clear_screenshot_card();
                                        cx.notify();
                                    }),
                                )),
                        ),
                )
                .child(div().h(px(1.0)).w_full().bg(gpui_kit::rgb(0x2c2c34)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(screenshot_card_action(
                            "screenshot-card-copy",
                            IconName::Copy,
                            platform::clipboard::COPY_LABEL,
                            cx.listener(|this, _: &ClickEvent, _window, cx| {
                                this.copy_screenshot(cx)
                            }),
                        ))
                        .child(card_divider())
                        .child(screenshot_card_action(
                            "screenshot-card-open",
                            IconName::ExternalLink,
                            "打开",
                            cx.listener(|this, _: &ClickEvent, _window, cx| {
                                this.open_screenshot(cx)
                            }),
                        ))
                        .child(card_divider())
                        .child(screenshot_card_action(
                            "screenshot-card-reveal",
                            IconName::FolderOpen,
                            "定位",
                            cx.listener(|this, _: &ClickEvent, _window, cx| {
                                this.reveal_screenshot(cx)
                            }),
                        )),
                ),
        )
    }

    fn render_error(&self, panel: f32, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let (title, message) = match (&self.error, &self.startup_failure) {
            (Some(banner), _) => (banner.title, banner.body.clone()),
            (None, Some(failure)) => ("播放出错", failure.clone()),
            (None, None) => return None,
        };
        Some(
            div()
                .id("error")
                .absolute()
                .left(px(24.0))
                .right(px(24.0 + panel))
                .bottom(px(96.0))
                .p(px(14.0))
                .rounded(px(10.0))
                .bg(gpui_kit::rgba(0x2a1414f2))
                .border_1()
                .border_color(gpui_kit::rgb(0x5a2424))
                // 点横幅（比如想选中文字）不该顺手暂停视频。
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(ui::danger())
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(text_muted())
                        .child(message),
                ),
        )
    }

    /// 行右键菜单：贴在鼠标位置，三项都是对「这一个文件」的操作。
    fn render_item_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let menu = self.item_menu.as_ref()?;
        let index = menu.index;
        let reveal_path = menu.path.clone();
        let info_path = menu.path.clone();

        Some(
            div()
                .id("item-menu")
                .absolute()
                .left(menu.position.x)
                .top(menu.position.y)
                .w(px(ITEM_MENU_WIDTH))
                .py(px(6.0))
                .rounded(px(10.0))
                .bg(surface())
                .border_1()
                .border_color(gpui_kit::rgb(0x2c2c34))
                .shadow_lg()
                // 菜单上的点击统一吃掉：一是别漏到画面去切播放状态，二是别让
                // 根节点的「点别处收菜单」把菜单在 mouse-down 阶段就拆掉，
                // 否则 mouse-up 时元素已经不在，菜单项永远点不中。
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_this, _: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                    }),
                )
                .child(item_menu_entry(
                    "item-menu-delete",
                    IconName::Trash,
                    "删除",
                    ui::danger(),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.item_menu = None;
                        this.remove_items(&[index], cx);
                    }),
                ))
                .child(item_menu_entry(
                    "item-menu-reveal",
                    IconName::FolderOpen,
                    "定位文件",
                    text(),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.reveal_item(reveal_path.clone(), cx);
                    }),
                ))
                .child(item_menu_entry(
                    "item-menu-info",
                    IconName::Info,
                    "查看格式",
                    text(),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.show_file_info(info_path.clone(), cx);
                    }),
                )),
        )
    }

    /// 「查看格式」面板：文件头 + 探测结果（容器、时长、大小、每条轨道）。
    fn render_file_info(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let panel = self.file_info.as_ref()?;
        let name = panel
            .path
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| panel.path.display().to_string());

        let body: AnyElement = match &panel.state {
            InfoState::Loading => {
                ui::label("正在读取格式信息…", 13.0, text_muted()).into_any_element()
            }
            InfoState::Failed(message) => div()
                .text_size(px(13.0))
                .text_color(ui::danger())
                .child(message.clone())
                .into_any_element(),
            InfoState::Ready(info) => render_media_info(info),
        };

        Some(
            div()
                .id("file-info")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui_kit::rgba(0x000000b8))
                // 面板是模态的：点它不该穿透去暂停视频。
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .child(
                    div()
                        .w(px(560.0))
                        .p(px(20.0))
                        .rounded(px(14.0))
                        .bg(surface())
                        .border_1()
                        .border_color(gpui_kit::rgb(0x2c2c34))
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .flex_1()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.0))
                                        .child(
                                            div()
                                                .text_size(px(15.0))
                                                .text_color(text())
                                                .child(name),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.0))
                                                .text_color(text_faint())
                                                .child(ui::ellipsis(
                                                    panel.path.display().to_string(),
                                                )),
                                        ),
                                )
                                .child(icon_button(
                                    "file-info-close",
                                    IconName::X,
                                    false,
                                    false,
                                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                                        this.file_info = None;
                                        cx.notify();
                                    }),
                                )),
                        )
                        .child(
                            div()
                                .id("file-info-body")
                                .max_h(px(430.0))
                                .overflow_y_scroll()
                                .child(body),
                        ),
                ),
        )
    }

    fn render_help(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.show_help {
            return None;
        }
        let rows = [
            ("空格", "播放 / 暂停"),
            ("← →", "快退 / 快进 5 秒"),
            ("↑ ↓", "音量"),
            ("滚轮", "音量"),
            ("Ctrl/⌘ 滚轮", "逐帧前进 / 后退"),
            ("N / P", "下一个 / 上一个"),
            ("[ ]", "减速 / 加速"),
            ("L", "播放列表"),
            ("Ctrl/⌘ 点击", "列表多选"),
            ("Shift 点击", "连选一段"),
            ("右键列表项", "删除 / 定位文件 / 查看格式"),
            ("Delete", "删除所选项"),
            ("M", "静音"),
            ("S", "截图"),
            ("O", "打开文件"),
            ("F / 双击", "全屏"),
            ("Esc", "退出全屏 / 关闭浮层"),
            ("?", "显示 / 隐藏本帮助"),
        ];
        Some(
            div()
                .id("help")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui_kit::rgba(0x000000b8))
                // 帮助是模态的：点它不该穿透去暂停视频。
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .child(
                    div()
                        .w(px(420.0))
                        .p(px(20.0))
                        .rounded(px(14.0))
                        .bg(surface())
                        .border_1()
                        .border_color(gpui_kit::rgb(0x2c2c34))
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .child(
                            div()
                                .text_size(px(15.0))
                                .text_color(text())
                                .child("键盘快捷键"),
                        )
                        .children(rows.iter().map(|(keys, action)| {
                            div()
                                .flex()
                                .justify_between()
                                .text_size(px(13.0))
                                .child(div().text_color(text_muted()).child(*keys))
                                .child(div().text_color(text()).child(*action))
                        })),
                ),
        )
    }
}

impl Render for PlayerApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.swap_frame(window);
        self.drop_retired_previews(window);
        self.update_render_target(window);

        let title = if self.snapshot.media_title.is_empty() {
            "视频播放器".to_string()
        } else {
            format!("{} · 视频播放器", self.snapshot.media_title)
        };
        window.set_window_title(&title);

        let fullscreen = window.is_fullscreen();
        // 全屏时把菜单栏让给鼠标：和底栏一起出现、一起消失，避免画面永久缩水。
        let bar_visible = !fullscreen || self.controls_visible();
        let video_top = if fullscreen {
            px(0.0)
        } else {
            px(MENUBAR_HEIGHT)
        };
        let osd_top = if fullscreen {
            px(28.0)
        } else {
            px(MENUBAR_HEIGHT + 16.0)
        };
        // 面板宽度只算一次：画面、控制条、面板本身都按同一个数让位。
        let panel_width = if self.settings.show_playlist {
            self.playlist_width(window)
        } else {
            0.0
        };

        div()
            .id("root")
            .relative()
            .size_full()
            .bg(bg())
            .text_color(text())
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &TogglePlay, _window, cx| this.toggle_play(cx)))
            .on_action(
                cx.listener(|this, _: &SeekForward, _window, cx| this.seek_by(SEEK_STEP, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SeekBackward, _window, cx| this.seek_by(-SEEK_STEP, cx)),
            )
            .on_action(
                cx.listener(|this, _: &VolumeUp, _window, cx| this.change_volume(VOLUME_STEP, cx)),
            )
            .on_action(
                cx.listener(|this, _: &VolumeDown, _window, cx| {
                    this.change_volume(-VOLUME_STEP, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleMute, _window, cx| this.toggle_mute(cx)))
            .on_action(cx.listener(|this, _: &ToggleFullscreen, window, cx| {
                this.toggle_fullscreen(window);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &TogglePlaylist, _window, cx| {
                this.set_playlist_visible(!this.settings.show_playlist);
                this.poke();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenFile, window, cx| {
                this.pick_file(window, cx);
            }))
            .on_action(cx.listener(|this, _: &NextFile, _window, cx| this.next_file(cx)))
            .on_action(cx.listener(|this, _: &PreviousFile, _window, cx| this.previous_file(cx)))
            .on_action(cx.listener(|this, _: &SpeedUp, _window, cx| {
                this.set_speed(this.snapshot.speed + 0.25, cx)
            }))
            .on_action(cx.listener(|this, _: &SpeedDown, _window, cx| {
                this.set_speed(this.snapshot.speed - 0.25, cx)
            }))
            .on_action(cx.listener(|this, _: &StopPlayback, _window, cx| {
                this.send(Command::Stop);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleHelp, _window, cx| this.toggle_help(cx)))
            .on_action(
                cx.listener(|this, _: &TakeScreenshot, _window, cx| this.take_screenshot(cx)),
            )
            .on_action(cx.listener(|this, _: &DeleteSelected, _window, cx| {
                // 面板关着的时候选择是看不见的，这时候删等于盲删，直接忽略。
                if this.settings.show_playlist {
                    this.remove_selection(cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &ExitOverlay, window, cx| this.exit_overlay(window, cx)),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                // `?` 不走键位表：GPUI 的键位串匹配的是**键帽字符**（`/` 而不是
                // `?`），而且不同键盘布局上 `?` 落在哪个键也不一样。直接看实际
                // 打出来的字符，`shift-/`、法语 AZERTY、德语 QWERTZ 都能命中。
                if event.keystroke.key_char.as_deref() == Some("?") {
                    this.toggle_help(cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    // 点任意别处先收起右键菜单；菜单项自己的点击会截断冒泡。
                    if this.item_menu.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    // 空白处右键同样收起菜单（行上的右键会截断，转去弹新菜单）。
                    if this.item_menu.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.poke();
                if this.dragging.is_some() {
                    this.update_progress_drag(event.position.x);
                }
                if this.volume_dragging {
                    this.set_volume_from_x(event.position.x);
                }
                if this.speed_dragging {
                    this.set_speed_from_x(event.position.x, cx);
                }
                if this.playlist_dragging {
                    this.drag_playlist_width(event.position.x, window);
                }
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _window, cx| {
                    if this.dragging.is_some() {
                        this.end_progress_drag(cx);
                    }
                    if this.volume_dragging {
                        this.volume_dragging = false;
                        this.settings.volume = this.snapshot.volume;
                    }
                    if this.speed_dragging {
                        this.speed_dragging = false;
                        this.settings.save();
                    }
                    if this.playlist_dragging {
                        // 宽度只在松手时落盘：拖动过程里每移动一像素写一次太浪费。
                        this.playlist_dragging = false;
                        this.settings.save();
                    }
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                let delta = match event.delta {
                    ScrollDelta::Pixels(delta) => f32::from(delta.y),
                    ScrollDelta::Lines(delta) => delta.y * 24.0,
                };
                if delta.abs() < 0.01 {
                    return;
                }
                // Ctrl/⌘ + 滚轮逐帧查看。滚轮按「事件」算，一格走一帧；触控板给的是
                // 像素增量，攒够一格才走，否则一次轻滑就是几十帧。
                if event.modifiers.secondary() {
                    let steps = match event.delta {
                        ScrollDelta::Lines(delta) => delta.y.signum() as i32,
                        ScrollDelta::Pixels(delta) => {
                            let accumulated = this.frame_scroll + f32::from(delta.y);
                            let steps = (accumulated / FRAME_SCROLL_PX).trunc();
                            // 只保留不足一格的余量，换方向时不会先欠着上一次的账。
                            this.frame_scroll = accumulated - steps * FRAME_SCROLL_PX;
                            steps as i32
                        }
                    };
                    if steps != 0 {
                        this.step_frame(steps, cx);
                    }
                    return;
                }
                this.frame_scroll = 0.0;
                this.change_volume(
                    if delta > 0.0 {
                        VOLUME_STEP
                    } else {
                        -VOLUME_STEP
                    },
                    cx,
                );
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _window, cx| {
                let dropped: Vec<PathBuf> = paths.0.iter().cloned().collect();
                this.open_paths(dropped, cx);
                this.poke();
                cx.notify();
            }))
            .child(self.render_video_area(video_top, panel_width, cx))
            .when(self.settings.show_playlist, |this| {
                this.child(self.render_playlist(video_top, panel_width, cx))
            })
            .when(self.controls_visible(), |this| {
                this.child(self.render_controls(panel_width, cx))
            })
            .children(self.render_menu(panel_width, cx))
            .children(self.render_osd(osd_top, panel_width))
            .children(self.render_screenshot_card(osd_top, cx))
            .when(bar_visible, |this| {
                this.child(self.render_menu_bar(window, cx))
            })
            .children(self.render_error(panel_width, cx))
            // 右键菜单在普通浮层之上、模态面板之下。
            .children(self.render_item_menu(cx))
            .children(self.render_help(cx))
            .children(self.render_file_info(cx))
    }
}

impl Drop for PlayerApp {
    fn drop(&mut self) {
        // 窗口关闭时必须让 mpv 停下来，否则音频线程会继续跑到进程退出。
        if let Some(engine) = &self.engine {
            engine.shutdown();
        }
        self.settings.save();
        // 列表本来就随改动落过盘，这里再存一次是兜住「拖入后一次都没点过」的情形。
        self.playlists.save();
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
