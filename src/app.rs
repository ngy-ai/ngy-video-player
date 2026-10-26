//! 主视图：窗口、输入、布局。
//!
//! 一条原则贯穿全文件：**界面是唯一的状态源**。mpv 只负责解码和输出帧，
//! 播放列表、循环模式、面板开合、拖动位置全部由这里持有。mpv 的属性变化只是
//! 「事实汇报」（现在第几秒、有没有暂停），收到后更新快照、触发重绘。

use crate::ui::IconName;
use crate::{
    formats,
    player::{Command, EndReason, Engine, LoopMode, PlayerEvent, Snapshot, TrackKind},
    playlist::{self, Playlist},
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
    App, Bounds, ClickEvent, Context, CursorStyle, ExternalPaths, FocusHandle, IntoElement,
    KeyBinding, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Render, RenderImage, ScrollDelta, ScrollWheelEvent, SharedString, Styled, Window, WindowBounds,
    WindowOptions, actions, canvas, div, point, px, size,
};
use std::{
    cell::Cell,
    collections::VecDeque,
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
        SpeedUp,
        SpeedDown,
        StopPlayback,
        ToggleHelp,
        ExitOverlay,
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

const SEEK_STEP: f64 = 5.0;
const VOLUME_STEP: f64 = 5.0;
const CONTROLS_IDLE: Duration = Duration::from_millis(2600);
const OSD_LIFETIME: Duration = Duration::from_millis(900);
const MENUBAR_HEIGHT: f32 = 32.0;

#[derive(Clone, Debug, Default)]
pub struct LaunchOptions {
    pub paths: Vec<PathBuf>,
    pub fullscreen: bool,
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

    playlist: Playlist,
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

    /// 原生文件选择框是否已经打开。
    ///
    /// rfd 的对话框阻塞在后台线程里，主窗口在拿到结果前不会知道自己「正在等对话框」。
    /// 不挡一下的话，连按 O 会叠出多个对话框，所以这里自己记一笔。
    dialog_open: bool,

    osd: Option<(Vec<String>, Instant)>,
    error: Option<String>,
    show_help: bool,
    /// 悬浮菜单（字幕 / 音轨 / 循环）当前展开的是哪一个。
    menu: Option<Menu>,
    /// 顶部菜单栏当前展开的是哪一个。
    menu_bar: Option<BarMenu>,

    startup_failure: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Menu {
    Subtitle,
    Audio,
    Speed,
    Loop,
}

/// 悬浮菜单里的一项点击后要做什么。
#[derive(Clone)]
enum MenuAction {
    Command(Command),
    Loop(LoopMode),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BarMenu {
    File,
    Playback,
    Subtitle,
    Audio,
    View,
    Help,
}

impl BarMenu {
    const ALL: [BarMenu; 6] = [
        BarMenu::File,
        BarMenu::Playback,
        BarMenu::Subtitle,
        BarMenu::Audio,
        BarMenu::View,
        BarMenu::Help,
    ];

    fn label(self) -> &'static str {
        match self {
            BarMenu::File => "文件",
            BarMenu::Playback => "播放",
            BarMenu::Subtitle => "字幕",
            BarMenu::Audio => "音轨",
            BarMenu::View => "视图",
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
            playlist: Playlist::default(),
            settings,
            focus,
            last_activity: Instant::now(),
            controls_forced: true,
            dragging: None,
            progress_bounds: Rc::new(Cell::new(Bounds::default())),
            volume_bounds: Rc::new(Cell::new(Bounds::default())),
            volume_dragging: false,
            dialog_open: false,
            osd: None,
            error: None,
            show_help: false,
            menu: None,
            menu_bar: None,
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

        if !options.paths.is_empty() {
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
                    PlayerEvent::Ended(reason) => {
                        this.on_ended(reason, cx);
                    }
                    PlayerEvent::Error(message) => {
                        log_warn!("播放器错误: {message}");
                        this.error = Some(message);
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
            || self.playlist.is_empty()
    }

    fn poke(&mut self) {
        self.last_activity = Instant::now();
    }

    fn toggle_help(&mut self, cx: &mut Context<Self>) {
        self.show_help = !self.show_help;
        cx.notify();
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

    fn set_loop(&mut self, mode: LoopMode, cx: &mut Context<Self>) {
        self.settings.loop_mode = mode;
        self.flash(mode.label());
        cx.notify();
    }

    fn toggle_fullscreen(&mut self, window: &mut Window) {
        window.toggle_fullscreen();
    }

    /// Esc 的语义：一层一层往回退，而不是直接全关。
    fn exit_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.show_help {
            self.show_help = false;
        } else if self.menu_bar.is_some() {
            self.menu_bar = None;
        } else if self.menu.is_some() {
            self.menu = None;
        } else if self.settings.show_playlist {
            self.settings.show_playlist = false;
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
        if reason == EndReason::Error {
            cx.notify();
            return;
        }
        match self.settings.loop_mode {
            LoopMode::One => {
                if let Some(path) = self.playlist.current().cloned() {
                    self.load_file(&path);
                }
            }
            LoopMode::All => {
                if let Some(next) = self.playlist.next_index(true) {
                    self.play_index(next);
                } else {
                    self.controls_forced = true;
                    cx.notify();
                }
            }
            LoopMode::Off => {
                if let Some(next) = self.playlist.next_index(false) {
                    self.play_index(next);
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

        self.playlist.replace(files, start);
        self.play_index(start);
    }

    fn play_index(&mut self, index: usize) {
        self.playlist.set_index(index);
        if let Some(path) = self.playlist.get(index).cloned() {
            self.load_file(&path);
        }
    }

    fn load_file(&mut self, path: &std::path::Path) {
        let url = path.to_string_lossy().to_string();
        self.snapshot.position = 0.0;
        self.snapshot.duration = 0.0;
        self.snapshot.tracks.clear();
        self.error = None;
        self.send(Command::Load { url, start: None });
        log_info!("打开 {}", path.display());
    }

    fn next_file(&mut self, cx: &mut Context<Self>) {
        let wrap = self.settings.loop_mode != LoopMode::Off || self.playlist.len() > 1;
        if let Some(index) = self.playlist.next_index(wrap) {
            self.play_index(index);
            cx.notify();
        }
    }

    fn previous_file(&mut self, cx: &mut Context<Self>) {
        let wrap = self.settings.loop_mode != LoopMode::Off || self.playlist.len() > 1;
        if let Some(index) = self.playlist.previous_index(wrap) {
            self.play_index(index);
            cx.notify();
        }
    }

    fn pick_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let (label, patterns) = formats::dialog_filter();
        // 挂到主窗口上：对话框是模态的，打开期间主窗口不接受输入。
        let mut dialog = rfd::FileDialog::new()
            .add_filter(&label, &[patterns.as_str()])
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
            .add_filter("字幕文件", &["*.srt;*.ass;*.ssa;*.vtt;*.sub;*.smi;*.txt"])
            .set_parent(window);
        if let Some(path) = self.playlist.current() {
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
        let area_width = f32::from(viewport.width) * scale;
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

    fn render_video_area(&self, top: Pixels, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("video-area")
            .absolute()
            .inset_0()
            .top(top)
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
                            if self.playlist.is_empty() {
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

    fn render_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
            .right_0()
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
                        self.playlist.len() <= 1,
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
                        self.playlist.len() <= 1,
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
                        self.menu == Some(Menu::Speed) || (snapshot.speed - 1.0).abs() > 0.001,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.menu = (this.menu != Some(Menu::Speed)).then_some(Menu::Speed);
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
                            this.settings.show_playlist = !this.settings.show_playlist;
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

    fn render_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let menu = self.menu?;
        let mut entries: Vec<(String, bool, Option<MenuAction>)> = Vec::new();

        match menu {
            Menu::Speed => {
                for speed in [0.5, 0.75, 1.0, 1.25, 1.5, 2.0] {
                    entries.push((
                        format_speed(speed),
                        (self.snapshot.speed - speed).abs() < 0.001,
                        Some(MenuAction::Command(Command::SetSpeed(speed))),
                    ));
                }
            }
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
                .right(px(18.0))
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
                                            MenuAction::Command(command) => this.send(command),
                                            MenuAction::Loop(mode) => this.set_loop(mode, cx),
                                        }
                                        this.menu = None;
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
                }),
        )
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

                let navigable = self.playlist.len() > 1;
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
                self.settings.show_playlist = !self.settings.show_playlist;
            }
            BarAction::ToggleFullscreen => self.toggle_fullscreen(window),
            BarAction::ToggleHelp => self.show_help = !self.show_help,
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
                        this.poke();
                        cx.notify();
                    }))
                    // 已经展开一个菜单时，移过其它标题直接切换，符合桌面菜单习惯。
                    .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                        if *hovered && this.menu_bar.is_some() && this.menu_bar != Some(menu) {
                            this.menu_bar = Some(menu);
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

    fn render_playlist(&self, top: Pixels, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.playlist.index();
        div()
            .id("playlist")
            .absolute()
            .top(top)
            .right_0()
            .bottom_0()
            .w(px(300.0))
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
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(14.0))
                    .py(px(12.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(text_muted())
                            .child(format!("播放列表 · {}", self.playlist.len())),
                    )
                    .child(icon_button(
                        "playlist-close",
                        IconName::X,
                        false,
                        false,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.settings.show_playlist = false;
                            cx.notify();
                        }),
                    )),
            )
            .child(
                div()
                    .id("playlist-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(
                        self.playlist
                            .items()
                            .iter()
                            .enumerate()
                            .map(|(index, path)| {
                                let name = path
                                    .file_name()
                                    .map(|value| value.to_string_lossy().to_string())
                                    .unwrap_or_else(|| path.display().to_string());
                                let is_current = current == Some(index);
                                div()
                                    .id(gpui_kit::ElementId::Name(format!("item-{index}").into()))
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .px(px(14.0))
                                    .py(px(9.0))
                                    .when(is_current, |this| this.bg(gpui_kit::rgb(0x1f2a3a)))
                                    .when(!is_current, |this| {
                                        this.hover(|style| style.bg(ui::surface_hover()))
                                    })
                                    .cursor(CursorStyle::PointingHand)
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .text_color(if is_current {
                                                ui::accent()
                                            } else {
                                                text()
                                            })
                                            .child(ui::ellipsis(name)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(text_faint())
                                            .child(formats::kind_label(path).to_string()),
                                    )
                                    .on_click(cx.listener(
                                        move |this, _: &ClickEvent, _window, cx| {
                                            this.play_index(index);
                                            cx.notify();
                                        },
                                    ))
                            }),
                    ),
            )
    }

    fn render_osd(&self, top: Pixels) -> Option<impl IntoElement> {
        let (lines, _) = self.osd.as_ref()?;
        Some(
            div()
                .absolute()
                .left(px(0.0))
                .right(px(0.0))
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

    fn render_error(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let message = self
            .error
            .clone()
            .or_else(|| self.startup_failure.clone())?;
        Some(
            div()
                .id("error")
                .absolute()
                .left(px(24.0))
                .right(px(24.0))
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
                        .child("播放出错"),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(text_muted())
                        .child(message),
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
            ("N / P", "下一个 / 上一个"),
            ("[ ]", "减速 / 加速"),
            ("L", "播放列表"),
            ("M", "静音"),
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
                this.settings.show_playlist = !this.settings.show_playlist;
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
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                this.poke();
                if this.dragging.is_some() {
                    this.update_progress_drag(event.position.x);
                }
                if this.volume_dragging {
                    this.set_volume_from_x(event.position.x);
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
            .child(self.render_video_area(video_top, cx))
            .when(self.settings.show_playlist, |this| {
                this.child(self.render_playlist(video_top, cx))
            })
            .when(self.controls_visible(), |this| {
                this.child(self.render_controls(cx))
            })
            .children(self.render_menu(cx))
            .children(self.render_osd(osd_top))
            .when(bar_visible, |this| {
                this.child(self.render_menu_bar(window, cx))
            })
            .children(self.render_error(cx))
            .children(self.render_help(cx))
    }
}

impl Drop for PlayerApp {
    fn drop(&mut self) {
        // 窗口关闭时必须让 mpv 停下来，否则音频线程会继续跑到进程退出。
        if let Some(engine) = &self.engine {
            engine.shutdown();
        }
        self.settings.save();
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
        KeyBinding::new("escape", ExitOverlay, None),
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
