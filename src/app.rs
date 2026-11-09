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

use crate::export::{ExportEvent, ExportTask};
use crate::ui::IconName;
use crate::{
    formats, platform,
    player::{
        Command, EndReason, Engine, EngineOptions, LoopMode, PlayerEvent, Snapshot, TrackKind,
    },
    playlist::{self, DropSpot, Playlist, PlaylistSet},
    settings::{DecodeMode, OnTop, RenderMode, Rotation, Settings, VideoAspect, normalize_speed},
    timeline::{
        AssetId, Project, Tick, UndoHistory, ViewState,
        autosave::{self, AutoSave},
        frame_pixels, seek_seconds, snap_to_targets, visible_frames, visible_ticks,
    },
    ui::{
        self, bg, format_time, format_time_precise, icon, icon_button, icon_button_with_tip,
        pill_button_with_tip, surface, text, text_faint, text_muted,
    },
};
use anyhow::Result;
use futures::{StreamExt as _, channel::mpsc::UnboundedReceiver};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, AnyWindowHandle, App, AppContext, Bounds, ClickEvent, Context, Corners,
    CursorStyle, ElementId, Entity, ExternalPaths, FocusHandle, Hsla, IntoElement, KeyBinding,
    KeyDownEvent, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, Render, RenderImage, ScrollDelta, ScrollEvent, SharedString, Size, Styled, WeakEntity,
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
        StepForward,
        StepBackward,
        VolumeUp,
        VolumeDown,
        ToggleMute,
        ToggleFullscreen,
        ToggleMini,
        TogglePlaylist,
        ToggleLibrary,
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
        RippleDeleteSelected,
        SetSourceIn,
        SetSourceOut,
        InsertFromSource,
        OverwriteFromSource,
        SubtitleEarlier,
        SubtitleLater,
        ToggleStats,
        SaveProject,
        SaveProjectAs,
        SplitPlayhead,
        ToggleMarker,
        SetInPoint,
        SetOutPoint,
        ZoomTimelineIn,
        ZoomTimelineOut,
        FitTimeline,
        UndoEdit,
        RedoEdit,
    ]
);

// 子模块按职责拆分：`PlayerApp` 的方法分散在各文件里，Rust 允许同一类型有多个 impl 块。
mod ab_loop;
mod associations;
mod bar;
mod bar_action;
mod clip;
mod concat;
mod constants;
mod controls;
mod convert;
mod delete_track_confirm;
mod export_queue;
mod format;
mod history;
mod import;
mod keys;
mod library;
mod menu_bar;
mod menus;
mod metadata;
mod mini;
mod multicam;
mod overlays;
mod picture;
mod playback;
mod playlist_menu;
mod playlist_ops;
mod playlist_sort;
mod playlist_ui;
mod project_autosave;
mod project_export;
mod project_file;
mod project_pack;
mod proxy;
mod remux;
mod render;
mod scenes;
mod scope;
mod sequence_dialog;
mod session;
mod settings_panel;
mod source_edit;
#[cfg(test)]
mod tests;
mod timeline_effects;
mod timeline_header;
mod timeline_keys;
mod timeline_menu;
mod timeline_preview;
mod timeline_speed;
mod timeline_tracks;
mod timeline_view;
mod video_area;
mod widgets;
mod window;

use self::bar_action::BarAction;
use self::clip::ClipDialog;
use self::concat::ConcatDialog;
use self::constants::*;
use self::convert::ConvertDialog;
use self::delete_track_confirm::DeleteTrackConfirm;
use self::export_queue::QueuedExport;
use self::format::*;
use self::import::PlaylistImportDialog;
pub use self::keys::key_bindings;
use self::library::{AssetDrop, LibraryImport, LibraryPanel};
use self::metadata::MetadataDialog;
use self::mini::MiniPlayer;
use self::multicam::MulticamDialog;
use self::playlist_menu::PanelMenu;
use self::project_file::ProjectConfirm;
use self::proxy::ProxyDialog;
use self::remux::RemuxDialog;
use self::scenes::ScenesDialog;
use self::scope::ScopeDialog;
use self::sequence_dialog::{SequenceDialog, SequencePurpose};
use self::settings_panel::SettingsTab;
use self::timeline_menu::TimelineMenu;
use self::timeline_tracks::TimelineDrop;
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
    /// 这一行是文件夹。菜单项不同：文件夹没有「查看格式」，多一项展开/收起。
    dir: bool,
    position: Point<Pixels>,
}

/// 播放列表页签的右键菜单。位置是窗口坐标，弹窗按它绝对定位。
struct TabMenu {
    index: usize,
    position: Point<Pixels>,
}

/// 正在重命名的列表页：改的是哪一页、已经输入了什么。
struct TabRename {
    index: usize,
    text: String,
    /// 用户已经动过键盘。预填的名字只是让人知道正在改哪一页，第一次敲字
    /// 就把它整个换掉——列表名基本是整条重写，不是接着往后补。
    typed: bool,
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

/// A-B 循环的两个标记点。
///
/// `path` 记下标记时的文件：换片后位置与时长都变了，旧标记必须失效，
/// 不能在别的片子上继续循环。
#[derive(Clone, Debug, Default)]
struct AbLoop {
    path: Option<PathBuf>,
    start: Option<f64>,
    end: Option<f64>,
    /// 上一次回跳的时刻，用来避开 seek 的延迟。
    seeked_at: Option<Instant>,
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
    /// 当前画面的代次。换片 / 停止后只接受同代次的帧，否则上一部片子渲染好、
    /// 却晚于清空事件才送达的帧会把旧画面贴回来。
    frame_epoch: u64,
    /// 本次装载的是纯音频文件：整个代次都不接受帧，视频区保持黑屏。
    /// 按扩展名先判一次，不必等 mpv 把轨道表报上来（那一步有滞后）。
    loading_audio_only: bool,

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
    /// 面板标题栏上展开着的弹出菜单（添加 / 排序），同一时刻只有一个。
    panel_menu: Option<PanelMenu>,
    /// 拖动列表行时算出来的落点：插到哪一行前面/后面，或者放进哪个文件夹。
    drop_spot: Option<DropSpot>,
    /// 播放列表行的右键菜单。
    item_menu: Option<ItemMenu>,
    /// 播放列表页签的右键菜单（重命名 / 删除）。
    tab_menu: Option<TabMenu>,
    /// 正在重命名的那一页；`None` 表示没有打开输入框。
    tab_rename: Option<TabRename>,
    /// 「查看格式」面板；`None` 表示没打开。
    file_info: Option<FileInfoPanel>,
    /// 裁切片段对话框；`None` 表示没打开。
    clip: Option<ClipDialog>,
    /// 裁切任务代次。后台任务回来时用它确认事件仍属于当前对话框，
    /// 否则关掉再打开一次，旧任务的完成事件会改写新对话框。
    clip_generation: u64,
    /// 格式转换面板；`None` 表示没打开。
    convert: Option<ConvertDialog>,
    /// 转换任务代次，作用同 `clip_generation`。扫描结果也认这个代次：
    /// 换文件夹或重开面板之后，上一次扫描的慢结果不该填进新面板。
    convert_generation: u64,
    /// A-B 循环的标记点。
    ab_loop: AbLoop,
    /// 字幕延迟（秒）。正数表示字幕延后出现。
    ///
    /// mpv 的 `sub-delay` 是逐文件的，换片会自动归零；界面这份要跟着一起清掉，
    /// 否则菜单里显示的数字和实际效果对不上。
    subtitle_delay: f64,
    /// 音轨 / 字幕编辑对话框；`None` 表示没打开。
    remux: Option<RemuxDialog>,
    /// 轨道编辑任务代次，作用同 `clip_generation`。
    remux_generation: u64,
    /// 章节 / 元数据编辑对话框；`None` 表示没打开。
    metadata: Option<MetadataDialog>,
    /// 元数据编辑任务代次，作用同 `clip_generation`。
    metadata_generation: u64,
    /// 代理生成对话框；`None` 表示没打开。
    proxy: Option<ProxyDialog>,
    /// 代理任务代次，作用同 `clip_generation`。
    proxy_generation: u64,
    /// 示波器对话框；`None` 表示没打开。
    scope: Option<ScopeDialog>,
    /// 示波器采样代次，作用同 `clip_generation`。
    scope_generation: u64,
    /// 镜头分割对话框；`None` 表示没打开。
    scenes: Option<ScenesDialog>,
    /// 镜头分割任务代次，作用同 `clip_generation`。
    scenes_generation: u64,
    /// 双机位导出对话框；`None` 表示没打开。
    multicam: Option<MulticamDialog>,
    /// 双机位任务代次，作用同 `clip_generation`。
    multicam_generation: u64,
    /// 拼接对话框；`None` 表示没打开。
    concat: Option<ConcatDialog>,
    /// 拼接任务代次，作用同 `clip_generation`。
    concat_generation: u64,
    settings: Settings,
    /// 上一次真正下发给窗口系统的置顶状态。
    ///
    /// 置顶是窗口系统的事，不是界面每帧都要重设的属性；比对一下，只在
    /// 「想置顶」和「已经在置顶」不一致时才调平台 API。
    on_top_applied: bool,
    focus: FocusHandle,

    /// 鼠标最后一次活动的时间，用来决定控制条什么时候淡出。
    last_activity: Instant,
    /// 程序启动时刻。自动保存的计时接口收的是「启动以来的时长」，单测里能直接快进。
    started: Instant,
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

    /// 工程模式：打开 `.ngyproj` 后才有；`None` 表示纯播放器。
    ///
    /// 工程模式下布局切成「上方预览 + 下方时间线」，右侧面板与控制条都不画。
    project: Option<Project>,
    /// 编辑撤销栈。只在工程模式下有，新建 / 打开工程时重置。
    history: Option<UndoHistory>,
    /// 编辑历史侧栏是否展开。贴在工程模式预览区右侧，不是模态。
    show_history: bool,
    /// 当前工程的落点；`None` 表示新建之后还没存过。
    project_path: Option<PathBuf>,
    /// 上一次保存 / 打开 / 新建时的内容快照，用来判断有没有未保存改动。
    ///
    /// 不用 `dirty: bool`：编辑操作分散在后面的任务里，逐个记得置位迟早会漏；
    /// 拿内容比对一次，不管从哪条路径改的都能反映出来。
    project_saved: Option<Project>,
    /// 自动保存的计时器：编辑后 30 秒没有新操作就落一份 `.autosave`。
    autosave: AutoSave,
    /// 「从播放列表导入」对话框；`None` 表示没打开。
    playlist_import: Option<PlaylistImportDialog>,
    /// 素材库面板（工程模式左侧）；`None` 表示没打开。
    library: Option<LibraryPanel>,
    /// 后台探测的进度；`None` 表示没有正在跑的导入。
    library_import: Option<LibraryImport>,
    /// 探测任务的代数：换工程之后，上一个任务的结果不能再写进新素材表。
    library_generation: u64,
    /// 正在试听的素材：预览区暂时放的是原片，不是时间线内容。
    audition: Option<AssetId>,
    /// 三点编辑的素材：源入出点是它的，试听结束（拖了播放头）也还留着。
    ///
    /// 与 `audition` 分开两笔账：`audition` 管「预览区现在放的是不是原片」，
    /// 只要挪一下播放头就结束；`source_asset` 管「我这一截要从哪个素材里取」，
    /// 得等真的放上时间线、换素材或者手动取消才清。
    source_asset: Option<AssetId>,
    /// 时间线的缩放与滚动。关掉工程也留着，下次打开还在原处。
    timeline: ViewState,
    /// 正在拖时间线的播放头。
    timeline_dragging: bool,
    /// 拖播放头时按住了 Shift：落点吸附到 0.01 秒。
    timeline_snap: bool,
    /// 拖动片段时算出来的落点；`None` 表示没在拖。
    timeline_drop: Option<TimelineDrop>,
    /// 从素材库往轨道上拖时算出来的落点；`None` 表示没在拖素材。
    asset_drop: Option<AssetDrop>,
    /// 时间线片段上的右键菜单；`None` 表示没展开。
    timeline_menu: Option<TimelineMenu>,
    /// 工程模式预览正在放的那个素材文件。同一个文件只 seek，换文件才重新加载。
    timeline_preview: Option<PathBuf>,
    /// 预览是不是在连续播放（播放头自己会走）。
    timeline_playing: bool,
    /// 上一次推进播放头的时刻。播放时钟按它与现在的差值算这一步走了多久。
    timeline_last_tick: Instant,
    /// 向 mpv 发过 `Load` / 纠正过 seek 的时刻。它之后的几百毫秒里位置不可信。
    timeline_loaded_at: Option<Instant>,
    /// 播放时钟的代数。每次开始播放加一，旧的那条循环看到代数变了就自己退出。
    timeline_clock: u64,
    /// 音频轨预览的第二个 mpv 实例。第一次真的有音频片段时才创建。
    timeline_audio: Option<crate::player::audio_preview::AudioPreview>,
    /// 音频实例里已经装着哪个文件；`None` 表示没装。
    timeline_audio_file: Option<PathBuf>,
    /// 是否正压着主画面自带的声音（音频轨片段优先）。
    timeline_audio_solo: bool,
    /// 正在跑的导出任务；`None` 表示空闲。
    export_task: Option<ExportTask>,
    /// 导出进度 0..=1，仅在 `export_task` 存在时有意义。
    export_progress: Option<f32>,
    /// 排队等着导出的任务。一次只跑一条，失败不阻塞后面的。
    export_queue: VecDeque<QueuedExport>,

    /// 原生文件选择框是否已经打开。
    ///
    /// rfd 的对话框阻塞在后台线程里，主窗口在拿到结果前不会知道自己「正在等对话框」。
    /// 不挡一下的话，连按 O 会叠出多个对话框，所以这里自己记一笔。
    ///
    /// 只记原生选择框；应用内模态各自有自己的字段（比如 `project_confirm`）。
    dialog_open: bool,
    /// 「工程还没有保存」的确认框；`None` 表示没在问。
    project_confirm: Option<ProjectConfirm>,
    /// 「删除轨道」确认框；`None` 表示没在问。轨道上有片段时才会弹出来。
    delete_track_confirm: Option<DeleteTrackConfirm>,
    /// 序列规格对话框；`None` 表示没在改。新建工程与改现有序列共用它。
    sequence_dialog: Option<SequenceDialog>,
    /// 试听原片时设的源入点 / 源出点。只对当前试听的素材有效，换素材或退出试听就清掉。
    /// 与时间线的工作区入出点（`project.ui.in_point` / `out_point`）是两回事。
    source_in: Option<Tick>,
    source_out: Option<Tick>,
    /// 还没打开的序列规格对话框。
    ///
    /// 「新建工程」是从菜单或关闭确认里来的，那两处手上没有 `window`，建不了输入框；
    /// 先记一笔，下一帧在 `render` 里开。
    pending_sequence_dialog: Option<SequencePurpose>,

    /// 播放统计浮层（帧率、丢帧计数）是否打开。按 I 切换。
    show_stats: bool,

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
    /// 进小窗之前主窗口是不是全屏。`open_mini` 会让主窗口退出全屏（不可见的
    /// 全屏窗口没有意义），还原时要把这个状态还回去，否则用户会觉得丢了全屏。
    mini_restore_fullscreen: bool,
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
enum BarMenu {
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
    const PLAYBACK: &'static [BarMenu] = &[
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
    const EDITING: &'static [BarMenu] = &[
        BarMenu::File,
        BarMenu::Edit,
        BarMenu::Preview,
        BarMenu::View,
        BarMenu::Settings,
        BarMenu::Help,
    ];

    fn for_mode(editing: bool) -> &'static [BarMenu] {
        if editing {
            Self::EDITING
        } else {
            Self::PLAYBACK
        }
    }

    fn label(self) -> &'static str {
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

/// 轨道编辑对话框处理哪一路。音轨与字幕分开成两个入口、两个对话框，
/// 共用同一套状态机与后台任务，只是标题、按钮和说明各按自己那一路来。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditTarget {
    Audio,
    Subtitle,
}

impl EditTarget {
    fn title(self) -> &'static str {
        match self {
            EditTarget::Audio => "编辑音轨",
            EditTarget::Subtitle => "内嵌字幕",
        }
    }

    /// 对话框里那一行的小标题。
    fn label(self) -> &'static str {
        match self {
            EditTarget::Audio => "音轨",
            EditTarget::Subtitle => "字幕",
        }
    }
}

/// 会丢掉当前工程的动作用它排队：有未保存改动时，先确认再执行。
#[derive(Clone, Debug, PartialEq, Eq)]
enum AfterSaveConfirm {
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
struct MiniState {
    title: SharedString,
    position: f64,
    duration: f64,
    playing: bool,
    volume: f64,
    muted: bool,
    /// 播放列表是否为空——空的时候画面区提示「没有正在播放的文件」。
    empty: bool,
    /// 当前播放页的条目数。小窗的上一集 / 下一集按钮据此决定是否可点。
    items: usize,
    /// 循环模式。右键菜单显示当前值，切换后小窗也要跟着更新。
    loop_mode: LoopMode,
    /// 播放速度。`[` / `]` 调完之后小窗要浮一条当前倍速。
    speed: f64,
    /// 当前有没有选中字幕轨。
    subtitle: bool,
    /// 文件里有没有字幕轨；没有时字幕按钮置灰。
    has_subtitle: bool,
    /// 正在播纯音频：没有画面时不显示「正在加载…」占位。
    audio_only: bool,
}

impl MiniState {
    /// 和另一份状态是否「看起来一样」。进度和音量按百分之一内视为没变，
    /// 免得每帧都判定成变化而白重绘一次小窗。
    fn same_as(&self, other: &Self) -> bool {
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

/// 打开主窗口。`options.paths` 里的文件会被直接加入播放列表并开始播放。
pub fn run(options: LaunchOptions) -> Result<()> {
    let settings = Settings::load();

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx: &mut App| {
            crate::logging::init();
            // 硬件编码器探测放后台：启用哪个编码器不阻塞界面，探测好之后
            // 下一次裁切就会自动用上。
            crate::encoder::warm_up();
            gpui_kit::init(cx);
            // 组件库默认是浅色主题；本项目整套界面是深色的，只用到它的输入框，
            // 强制深色至少不会在元数据对话框里出现一块白色输入区。
            gpui_kit::component::Theme::change(gpui_kit::component::ThemeMode::Dark, None, cx);
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
                    // 标题栏的 × 也要走未保存确认（见 `app/window.rs`）。
                    self::window::guard_window_close(view.clone(), window, cx);
                    view
                },
            )
            .expect("无法创建窗口");
        });

    Ok(())
}
