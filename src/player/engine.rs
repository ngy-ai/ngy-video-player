//! 播放器引擎：libmpv 的 mpv_handle 与渲染上下文。
//!
//! 线程划分：
//!
//! ```text
//!  ┌────────────┐  Command   ┌──────────────┐  property/event  ┌────────────┐
//!  │  GPUI 界面 │ ─────────► │ 引擎线程      │ ───────────────► │ 共享快照   │
//!  │  (主线程)  │ ◄───────── │ wait_event   │                  └────────────┘
//!  └────────────┘  PlayerEvent└──────────────┘
//!        ▲                          │ mpv_handle
//!        │ Frame(RenderImage)       ▼
//!  ┌────────────┐          ┌──────────────┐
//!  │ 取帧结果   │ ◄──────── │ 渲染线程      │  ← 软件渲染，不需要 GL 上下文
//!  └────────────┘          └──────────────┘
//! ```
//!
//! 为什么用软件渲染而不是 OpenGL/Vulkan 互通：GPUI 用的是 wgpu，把 mpv 的 GL 纹理
//! 共享给 wgpu 需要同一套设备/上下文，三大平台的窗口系统各有一堆边界情况。软件渲染
//! 每个显示帧只付出一次拷贝的代价，换来的是「一份代码在三个平台行为完全一致」，对于
//! 播放器这种以正确性优先的产品，这笔交易划算。真正的高码率 4K 场景再另做零拷贝
//! 后端即可，接口不需要变。

use crate::player::{
    ffi::{
        self, MpvApi, MpvEventEndFile, MpvEventProperty, MpvRenderContext, MpvRenderParam, SendPtr,
        event, format, render_param,
    },
    frame::{self, FrameBuffer},
    thumbnail::{self, Thumbnailer},
};
use anyhow::{Context as _, Result, anyhow};
use futures::channel::mpsc::UnboundedSender;
use gpui_kit::RenderImage;
use std::{
    ffi::{CString, c_char, c_int, c_void},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

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

struct Shared {
    state: Mutex<Snapshot>,
    events: UnboundedSender<PlayerEvent>,
    shutdown: AtomicBool,
    /// 渲染目标尺寸（物理像素）。界面每次布局确定后写一次。
    target: Mutex<TargetSize>,
    /// 渲染线程每次重绘后自增，界面据此判断画面是否已经更新过。
    frame_serial: AtomicU64,
    /// 是否有界面在等待新帧（没有就跳过渲染，省电）。
    needs_frames: AtomicBool,
    /// 是否有一帧已经推给界面但还没被取走。
    ///
    /// 界面渲染比视频帧率慢时（窗口被挡住、系统卡顿），如果不做背压，
    /// 未消费的 `RenderImage` 会在通道里堆积，每个都是几 MB 的像素。
    /// 这里宁可丢帧，也不让内存膨胀。
    frame_pending: AtomicBool,
    /// 视频原始尺寸变化计数器，避免每帧都读两个属性。
    config_serial: AtomicU32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TargetSize {
    width: u32,
    height: u32,
}

impl Default for TargetSize {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
        }
    }
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, Snapshot> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn notify(&self, event: PlayerEvent) {
        let _ = self.events.unbounded_send(event);
    }
}

pub struct Engine {
    api: Arc<MpvApi>,
    handle: SendPtr<ffi::MpvHandle>,
    shared: Arc<Shared>,
    commands: std::sync::mpsc::Sender<Command>,
    signal: Arc<RenderSignal>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    render_target: Mutex<TargetSize>,
    /// 进度条悬停预览用的第二个 mpv 实例。
    ///
    /// 第一次悬停时才创建：大多数会话根本不会碰进度条，没必要一启动就多开
    /// 一份解码器。`None` 且 `thumbnail_failed` 为真是「试过、起不来」。
    thumbnail: Mutex<Option<Thumbnailer>>,
    thumbnail_failed: AtomicBool,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine").finish_non_exhaustive()
    }
}

impl Engine {
    /// 起引擎。`events` 是界面侧的接收端对应的发送端。
    pub fn spawn(events: UnboundedSender<PlayerEvent>) -> Result<Self> {
        let api = MpvApi::load()?;
        let shared = Arc::new(Shared {
            state: Mutex::new(Snapshot::default()),
            events,
            shutdown: AtomicBool::new(false),
            target: Mutex::new(TargetSize::default()),
            frame_serial: AtomicU64::new(0),
            needs_frames: AtomicBool::new(false),
            frame_pending: AtomicBool::new(false),
            config_serial: AtomicU32::new(0),
        });

        let handle = unsafe { (api.create)() };
        if handle.is_null() {
            return Err(anyhow!("mpv_create 返回空指针"));
        }
        let handle = SendPtr(handle);

        unsafe {
            Self::configure(&api, handle.get()).context("初始化 libmpv 配置")?;

            let rc = (api.initialize)(handle.get());
            if rc < 0 {
                let message = api.error_text(rc);
                (api.terminate_destroy)(handle.get());
                return Err(anyhow!("mpv_initialize 失败: {message}"));
            }
        }

        let (commands, command_rx) = std::sync::mpsc::channel();
        let signal = Arc::new(RenderSignal::default());

        let engine_shared = Arc::clone(&shared);
        let engine_api = Arc::clone(&api);
        let engine_handle = SendPtr(handle.get());
        let engine_thread = std::thread::Builder::new()
            .name("mpv-events".into())
            .spawn(move || {
                event_loop(engine_api, engine_handle.get(), engine_shared, command_rx);
            })
            .context("启动 mpv 事件线程")?;

        let render_shared = Arc::clone(&shared);
        let render_api = Arc::clone(&api);
        let render_handle = SendPtr(handle.get());
        let render_signal = Arc::clone(&signal);
        let render_thread = std::thread::Builder::new()
            .name("mpv-render".into())
            .spawn(move || {
                render_loop(
                    render_api,
                    render_handle.get(),
                    render_shared,
                    render_signal,
                );
            })
            .context("启动取帧线程")?;

        Ok(Self {
            api,
            handle,
            shared,
            commands,
            signal,
            threads: Mutex::new(vec![engine_thread, render_thread]),
            render_target: Mutex::new(TargetSize::default()),
            thumbnail: Mutex::new(None),
            thumbnail_failed: AtomicBool::new(false),
        })
    }

    /// mpv 侧的初始配置。这里的每一项都在解决一个具体问题，见各自注释。
    ///
    /// 选项被拒绝只记一条 warn：不同 mpv 版本认识的选项不完全一样，
    /// 一个装饰性选项（比如字幕字号）不该让整个播放器起不来。
    unsafe fn configure(api: &MpvApi, handle: *mut ffi::MpvHandle) -> Result<()> {
        unsafe fn opt(api: &MpvApi, handle: *mut ffi::MpvHandle, name: &str, value: &str) {
            let Ok(cname) = CString::new(name) else {
                return;
            };
            let Ok(cvalue) = CString::new(value) else {
                return;
            };
            let rc = unsafe { (api.set_option_string)(handle, cname.as_ptr(), cvalue.as_ptr()) };
            if rc < 0 {
                log_warn!("mpv 不接受选项 {name}={value}: {}", api.error_text(rc));
            }
        }

        unsafe {
            // **必须有这一行**：不指定时 mpv 用默认的 `gpu-next` 视频输出，
            // 自己建一个隐藏窗口消费所有帧，render API 的
            // `mpv_render_context_update()` 就永远返回 0——表现是界面一直黑着，
            // 但播放进度正常推进。`libmpv` 这个 VO 才会把帧交给 render context。
            opt(api, handle, "vo", "libmpv");
            //
            // 音频仍然交给 mpv：音视频同步由它负责，我们不用自己对齐时钟。
            //
            // 但**不要**去设置 `ao`：`ao` 接受的是驱动名列表（wasapi、coreaudio、
            // pipewire/pulse/alsa……），并没有一个叫 `auto` 的驱动。写成 `ao=auto`
            // 时 mpv 会报 `Audio output auto not found!`，然后按
            // `audio-fallback-to-null` 退到 null 输出——画面正常、进度正常，就是没有
            // 声音。留空才是「按平台优先级自动挑」,这里刻意不写。
            //
            // 没有可用声卡时（远程桌面、容器、拔掉的 USB 声卡）继续放画面，
            // 而不是让整个播放因为音频后端起不来而失败。
            opt(api, handle, "audio-fallback-to-null", "yes");
            // 不建自己的窗口。
            opt(api, handle, "force-window", "no");
            // 起播就绪即可，不要一上来缓冲一大段（本地文件场景纯属拖慢）。
            opt(api, handle, "cache", "auto");
            opt(api, handle, "demuxer-max-bytes", "128MiB");
            // 字幕：默认找同名外挂字幕，接受常见编码猜测。
            opt(api, handle, "sub-auto", "fuzzy");
            opt(api, handle, "sub-codepage", "auto");
            // 字体与样式：mpv 的 libass 会把字幕直接画进画面，我们不需要自己排版。
            opt(api, handle, "sub-font-size", "46");
            opt(api, handle, "sub-border-size", "2.2");
            opt(api, handle, "sub-shadow-offset", "0");
            // 初始值按「控制条可见」给，界面之后会根据控制条显隐在 104/44 之间切。
            opt(api, handle, "sub-margin-y", "104");
            // 音量上限放到 130%，给「片源太小声」留余地；超过 100% 走软放大。
            opt(api, handle, "volume-max", "130");
            // 保持画面比例由我们自己的布局负责，不要 mpv 再缩放一次。
            opt(api, handle, "video-zoom", "0");
            opt(api, handle, "keepaspect-window", "no");
            // 逐帧后退（frame-back-step）要求精确 seek：`hr-seek` 默认只在部分
            // 场景下精确，留着默认值会让「后退一帧」按下去画面不动。本地文件
            // 解码够快，全程精确的代价可以接受。
            opt(api, handle, "hr-seek", "yes");
            // 播放结束不要把文件卸掉：mpv 默认在 EOF 后清空播放列表，之后再按
            // 方向键 seek 会直接失败（"error running command"），画面也变成
            // 空窗。保留最后一帧更接近用户对播放器的预期。
            opt(api, handle, "keep-open", "yes");
            // 循环由播放列表逻辑控制，避免 mpv 自己把单曲循环掉。
            opt(api, handle, "loop-file", "no");
            opt(api, handle, "loop-playlist", "no");
            // 打开就播（由界面决定是否立刻暂停）。
            opt(api, handle, "pause", "yes");
            // 保证空闲时不会自动退出。
            opt(api, handle, "idle", "yes");
            opt(api, handle, "terminal", "no");
            opt(api, handle, "input-default-bindings", "no");
            opt(api, handle, "input-vo-keyboard", "no");

            if ffi::verbose_logging() {
                (api.request_log_messages)(handle, c"info".as_ptr());
            }
        }
        Ok(())
    }

    pub fn api(&self) -> &Arc<MpvApi> {
        &self.api
    }

    pub fn send(&self, command: Command) {
        if self.shared.shutdown.load(Ordering::SeqCst) {
            return;
        }
        let _ = self.commands.send(command);
        // 打断 wait_event，让命令立刻被处理（否则最多等一个超时周期）。
        unsafe { (self.api.wakeup)(self.handle.get()) };
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.state().clone()
    }

    pub fn frame_serial(&self) -> u64 {
        self.shared.frame_serial.load(Ordering::SeqCst)
    }

    /// 界面取走一帧后调用，允许渲染线程继续产出。
    pub fn frame_consumed(&self) {
        self.shared.frame_pending.store(false, Ordering::SeqCst);
    }

    /// 界面在布局确定后调用：告诉取帧线程应该按多大尺寸渲染。
    ///
    /// 尺寸的比例**必须与视频区域一致**，mpv 才会在该区域内按视频自身比例
    /// 排黑边；调用方负责按「不超过视频原始分辨率」等比缩放，这里只做范围夹取。
    pub fn set_render_target(&self, width: u32, height: u32) {
        let next = TargetSize {
            width: width.clamp(2, 7680),
            height: height.clamp(2, 4320),
        };
        let mut current = self.render_target.lock().unwrap_or_else(|e| e.into_inner());
        if *current != next {
            *current = next;
            *self.shared.target.lock().unwrap_or_else(|e| e.into_inner()) = next;
            self.signal.wake();
        }
    }

    pub fn target_size(&self) -> (u32, u32) {
        let target = *self.render_target.lock().unwrap_or_else(|e| e.into_inner());
        (target.width, target.height)
    }

    pub fn set_needs_frames(&self, needs: bool) {
        self.shared.needs_frames.store(needs, Ordering::SeqCst);
        if needs {
            self.signal.wake();
        }
    }

    pub fn toggle_fullscreen_hint(&self) {}

    /// 请求一张 `path` 在 `time` 处的预览图。
    ///
    /// 悬停时指针每动一下就会调一次：真正的节流在预览实例那侧（只做最新的一条），
    /// 这里只负责「第一次用的时候把实例起起来」。
    pub fn preview(&self, path: &Path, time: f64) {
        if self.shared.shutdown.load(Ordering::SeqCst)
            || self.thumbnail_failed.load(Ordering::Relaxed)
        {
            return;
        }

        // 渲染尺寸按视频比例走，宽银幕和 4:3 都不会被拉变形。
        let aspect = self.shared.state().aspect();
        let (_, height) = thumbnail::render_size(aspect);

        let mut slot = self.thumbnail.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            match Thumbnailer::spawn(Arc::clone(&self.api), self.shared.events.clone()) {
                Ok(created) => *slot = Some(created),
                Err(error) => {
                    // 只影响悬停预览这一个功能，所以记一条日志就够，
                    // 不弹横幅、也不影响正在播放的视频。
                    log_warn!("进度条预览不可用: {error:#}");
                    self.thumbnail_failed.store(true, Ordering::Relaxed);
                    return;
                }
            }
        }
        if let Some(thumbnailer) = slot.as_ref() {
            thumbnailer.show(path, time, height);
        }
    }

    /// 不再需要预览（换片、停止播放）：让预览实例把文件卸掉。
    pub fn stop_preview(&self) {
        if let Some(thumbnailer) = self
            .thumbnail
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            thumbnailer.stop();
        }
    }

    pub fn shutdown(&self) {
        if self.shared.shutdown.swap(true, Ordering::SeqCst) {
            return;
        }
        self.signal.wake();
        unsafe { (self.api.wakeup)(self.handle.get()) };

        // 预览实例先停：它独立于主实例，但同样占着文件和一路解码器。
        drop(
            self.thumbnail
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take(),
        );

        let threads = std::mem::take(&mut *self.threads.lock().unwrap_or_else(|e| e.into_inner()));
        for thread in threads {
            let _ = thread.join();
        }
        unsafe { (self.api.terminate_destroy)(self.handle.get()) };
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ---------------------------------------------------------------------------
// 事件线程
// ---------------------------------------------------------------------------

fn event_loop(
    api: Arc<MpvApi>,
    handle: *mut ffi::MpvHandle,
    shared: Arc<Shared>,
    commands: std::sync::mpsc::Receiver<Command>,
) {
    unsafe {
        // 订阅所有界面关心的属性。id 用来在回包里区分是哪一个。
        let watched: &[(&str, i32, u64)] = &[
            ("pause", format::FLAG, 1),
            ("time-pos", format::DOUBLE, 2),
            ("duration", format::DOUBLE, 3),
            ("volume", format::DOUBLE, 4),
            ("mute", format::FLAG, 5),
            ("speed", format::DOUBLE, 6),
            ("idle-active", format::FLAG, 7),
            ("paused-for-cache", format::FLAG, 8),
            ("seeking", format::FLAG, 9),
            ("media-title", format::STRING, 10),
            ("path", format::STRING, 11),
            ("width", format::INT64, 12),
            ("height", format::INT64, 13),
            ("eof-reached", format::FLAG, 14),
            // 只关心「变了」这个信号，具体内容变化后重新整块读。
            // 外挂字幕（sub-auto / sub-add）是在文件加载完之后才挂进轨道表的，
            // 只在 FILE_LOADED 读一次会永远漏掉它，字幕菜单就是空的。
            ("track-list", format::NONE, 15),
        ];
        for (name, fmt, id) in watched {
            if let Err(error) = api.observe(handle, *id, name, *fmt) {
                log_warn!("监听属性失败: {error}");
            }
        }

        let mut last_position_emit = Instant::now();

        loop {
            if shared.shutdown.load(Ordering::SeqCst) {
                break;
            }

            // 命令优先：用户刚点了按钮，不应该等事件超时。
            let mut quit = false;
            while let Ok(command) = commands.try_recv() {
                if matches!(command, Command::Stop) {
                    // Stop 之后若还有命令要处理，继续走，不做特殊跳出。
                }
                dispatch(&api, handle, &shared, command, &mut quit);
            }
            if quit {
                break;
            }

            let event_ptr = (api.wait_event)(handle, 0.05);
            if event_ptr.is_null() {
                continue;
            }
            let event = &*event_ptr;

            match event.event_id {
                event::SHUTDOWN => break,
                event::PROPERTY_CHANGE => {
                    if event.reply_userdata == 15 {
                        refresh_metadata(&api, handle, &shared);
                        shared.notify(PlayerEvent::Dirty);
                        continue;
                    }
                    if let Some(change) = property_change(api.as_ref(), event) {
                        let mut dirty = true;
                        let mut state = shared.state();
                        apply_property(&mut state, &change, &mut dirty);
                        drop(state);
                        if dirty {
                            // 进度更新很频繁，但界面本来就要为每一帧重绘；
                            // 只在暂停/空闲等场景才需要额外节流。
                            let _ = last_position_emit;
                            last_position_emit = Instant::now();
                            shared.notify(PlayerEvent::Dirty);
                        }
                    }
                }
                event::FILE_LOADED => {
                    refresh_metadata(&api, handle, &shared);
                    shared.notify(PlayerEvent::Dirty);
                }
                event::VIDEO_RECONFIG | event::AUDIO_RECONFIG => {
                    refresh_metadata(&api, handle, &shared);
                    shared.notify(PlayerEvent::Dirty);
                }
                event::PLAYBACK_RESTART => {
                    refresh_metadata(&api, handle, &shared);
                    shared.notify(PlayerEvent::Dirty);
                }
                event::END_FILE => {
                    let reason = if event.data.is_null() {
                        EndReason::Eof
                    } else {
                        let data = &*(event.data as *const MpvEventEndFile);
                        match data.reason {
                            ffi::end_file::EOF => EndReason::Eof,
                            ffi::end_file::STOP => EndReason::Stopped,
                            ffi::end_file::QUIT => EndReason::Quit,
                            ffi::end_file::ERROR => {
                                let message = if data.error < 0 {
                                    api.error_text(data.error)
                                } else {
                                    "未知错误".into()
                                };
                                shared.notify(PlayerEvent::Error(message));
                                EndReason::Error
                            }
                            _ => EndReason::Eof,
                        }
                    };
                    shared.notify(PlayerEvent::Ended(reason));
                    shared.notify(PlayerEvent::Dirty);
                }
                event::LOG_MESSAGE => {
                    if !event.data.is_null() {
                        let message = &*(event.data as *const ffi::MpvEventLogMessage);
                        let text = cstr(message.text);
                        let prefix = cstr(message.prefix);
                        log_debug!("[mpv/{prefix}] {text}");
                    }
                }
                _ => {}
            }
        }
    }
}

unsafe fn cstr(ptr: *const c_char) -> String {
    if ptr.is_null() {
        String::new()
    } else {
        unsafe { std::ffi::CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
}

struct PropertyChange {
    id: u64,
    format: i32,
    data: *mut c_void,
}

fn property_change(api: &MpvApi, event: &ffi::MpvEvent) -> Option<PropertyChange> {
    if event.data.is_null() {
        return None;
    }
    let property = unsafe { &*(event.data as *const MpvEventProperty) };
    let id = event.reply_userdata;
    if id == 0 {
        return None;
    }
    let _ = api;
    Some(PropertyChange {
        id,
        format: property.format,
        data: property.data,
    })
}

fn apply_property(state: &mut Snapshot, change: &PropertyChange, dirty: &mut bool) {
    if change.format == format::NONE || change.data.is_null() {
        // 属性变成不可用（比如切歌瞬间 time-pos 消失），保持旧值即可。
        return;
    }
    match change.id {
        1 => state.paused = flag(change),
        2 => state.position = double(change).unwrap_or(state.position),
        3 => state.duration = double(change).unwrap_or(state.duration),
        4 => state.volume = double(change).unwrap_or(state.volume),
        5 => state.muted = flag(change),
        6 => state.speed = double(change).unwrap_or(state.speed),
        7 => state.idle = flag(change),
        8 => state.buffering = flag(change),
        9 => state.seeking = flag(change),
        10 => state.media_title = string(change).unwrap_or_default(),
        11 => {
            let path = string(change).unwrap_or_default();
            state.path = (!path.is_empty()).then(|| PathBuf::from(path));
        }
        12 => state.video_width = int(change).unwrap_or(0).max(0) as u32,
        13 => state.video_height = int(change).unwrap_or(0).max(0) as u32,
        14 => {}
        _ => {
            *dirty = false;
        }
    }
}

fn flag(change: &PropertyChange) -> bool {
    unsafe { *(change.data as *const c_int) != 0 }
}

fn double(change: &PropertyChange) -> Option<f64> {
    unsafe { Some(*(change.data as *const f64)) }
}

fn int(change: &PropertyChange) -> Option<i64> {
    unsafe { Some(*(change.data as *const i64)) }
}

fn string(change: &PropertyChange) -> Option<String> {
    let ptr = unsafe { *(change.data as *const *const c_char) };
    if ptr.is_null() {
        None
    } else {
        Some(
            unsafe { std::ffi::CStr::from_ptr(ptr) }
                .to_string_lossy()
                .into_owned(),
        )
    }
}

/// 文件加载后把「一次性」的元数据整块读出来：轨道、章节、时长。
fn refresh_metadata(api: &Arc<MpvApi>, handle: *mut ffi::MpvHandle, shared: &Arc<Shared>) {
    let track_json = api.property_string(handle, "track-list");
    let chapter_json = api.property_string(handle, "chapter-list");

    let mut tracks = Vec::new();
    if let Some(raw) = track_json {
        for value in ffi::parse_track_list(&raw) {
            let kind = match ffi::json_str(&value, "type").as_deref() {
                Some("video") => TrackKind::Video,
                Some("audio") => TrackKind::Audio,
                Some("sub") => TrackKind::Subtitle,
                _ => continue,
            };
            tracks.push(Track {
                id: ffi::json_i64(&value, "id").unwrap_or(-1),
                kind,
                title: ffi::json_str(&value, "title").unwrap_or_default(),
                lang: ffi::json_str(&value, "lang").unwrap_or_default(),
                codec: ffi::json_str(&value, "codec").unwrap_or_default(),
                selected: value
                    .get("selected")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                external: value
                    .get("external")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                default: value
                    .get("default")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            });
        }
    }

    let mut chapters = Vec::new();
    if let Some(raw) = chapter_json {
        if let Ok(values) = serde_json::from_str::<Vec<serde_json::Value>>(&raw) {
            for value in values {
                chapters.push(Chapter {
                    title: ffi::json_str(&value, "title").unwrap_or_else(|| "未命名章节".into()),
                    time: ffi::json_f64(&value, "time").unwrap_or(0.0),
                });
            }
        }
    }

    let duration = api.property_f64(handle, "duration").unwrap_or(0.0);
    let width = api.property_i64(handle, "width").unwrap_or(0);
    let height = api.property_i64(handle, "height").unwrap_or(0);

    // 实际选中的音频输出。`null` 就是「画面在放、就是没声音」，
    // 原因通常是没声卡 / 音频后端起不来；这条日志是唯一能事后排查的线索。
    let audio_output = api
        .property_string(handle, "current-ao")
        .unwrap_or_default();

    let mut state = shared.state();
    state.tracks = tracks;
    state.chapters = chapters;
    if duration > 0.0 {
        state.duration = duration;
    }
    if width > 0 && height > 0 {
        state.video_width = width as u32;
        state.video_height = height as u32;
    } else {
        state.video_width = 0;
        state.video_height = 0;
    }
    if !audio_output.is_empty() && state.audio_device != audio_output {
        if audio_output == "null" {
            log_warn!(
                "音频输出退化为 null，当前文件不会有声音：\
                 检查系统默认播放设备，或把系统声音输出切回本机扬声器"
            );
        } else {
            log_info!("音频输出: {audio_output}");
        }
        state.audio_device = audio_output;
    }
    drop(state);

    shared.config_serial.fetch_add(1, Ordering::SeqCst);
}

fn dispatch(
    api: &Arc<MpvApi>,
    handle: *mut ffi::MpvHandle,
    shared: &Arc<Shared>,
    command: Command,
    quit: &mut bool,
) {
    // 哪些命令失败该让用户看见：只有「用户明确要求的动作没做成」才算。
    // seek/选轨/设置这类失败在边界状态下很常见（片头片尾、流不可跳转），
    // 它们只写日志——否则只是按了一下方向键，界面上就弹一条「播放出错」。
    let report_failure = matches!(command, Command::Load { .. } | Command::AddSubtitle(_));
    let result = (|| -> Result<()> {
        match command {
            Command::Load { url, start } => {
                {
                    let mut state = shared.state();
                    state.error = None;
                    state.position = 0.0;
                    state.duration = 0.0;
                    state.tracks.clear();
                    state.chapters.clear();
                }
                api.run_command(handle, &["loadfile", &url, "replace"])?;
                if let Some(seconds) = start {
                    api.run_command(handle, &["seek", &seconds.to_string(), "absolute"])?;
                }
                // 必须用命令、不能用 `set_property`：
                // `--pause` 是「打开文件后暂停」的选项，mpv 每次 loadfile 都会把它
                // 重新应用一遍；而 `set_property` 是即时的，会抢在排队的 loadfile
                // 之前执行完，于是文件加载时又被暂停回去——表现就是画面永远黑着。
                // 排进同一条命令队列才能保证「先 loadfile，再解除暂停」。
                api.run_command(handle, &["set", "pause", "no"])?;
            }
            Command::Stop => {
                api.run_command(handle, &["stop"])?;
            }
            Command::PlayPause => {
                // 停在最后一帧时（keep-open）mpv 不会自己回到开头，按播放要显式
                // 归零，否则用户会觉得「播完了按空格没反应」。
                if api.property_flag(handle, "eof-reached").unwrap_or(false) {
                    api.run_command(handle, &["seek", "0", "absolute"])?;
                    api.set_flag(handle, "pause", false)?;
                } else {
                    let paused = api.property_flag(handle, "pause").unwrap_or(false);
                    api.set_flag(handle, "pause", !paused)?;
                }
            }
            Command::SetPaused(paused) => api.set_flag(handle, "pause", paused)?,
            Command::SeekAbsolute(seconds) => {
                let target = seconds.max(0.0);
                let ended = api.property_flag(handle, "eof-reached").unwrap_or(false);
                api.run_command(handle, &["seek", &target.to_string(), "absolute", "exact"])?;
                // 结尾处（keep-open 会把播放停住）往回拖通常是想接着看，
                // 这里顺手解除停住；平时暂停着拖进度条则保持暂停。
                if ended {
                    api.set_flag(handle, "pause", false)?;
                }
            }
            Command::SeekRelative(delta) => {
                let ended = api.property_flag(handle, "eof-reached").unwrap_or(false);
                api.run_command(handle, &["seek", &delta.to_string(), "relative", "exact"])?;
                if ended {
                    api.set_flag(handle, "pause", false)?;
                }
            }
            Command::StepFrame(steps) => {
                // mpv 只有「走一帧」的命令，没有「走 N 帧」；滚轮一格算一帧，
                // 触控板攒出来的多帧就按次数重复执行。`frame-step` 自带暂停，
                // 但界面已经先发了 SetPaused，这里不再重复设置。
                let command = if steps >= 0 {
                    "frame-step"
                } else {
                    "frame-back-step"
                };
                for _ in 0..steps.unsigned_abs() {
                    api.run_command(handle, &[command])?;
                }
            }
            Command::SetVolume(volume) => {
                api.set_f64(handle, "volume", volume.clamp(0.0, 130.0))?;
            }
            Command::SetMuted(muted) => api.set_flag(handle, "mute", muted)?,
            Command::SetSpeed(speed) => {
                api.set_f64(handle, "speed", speed.clamp(0.1, 8.0))?;
            }
            Command::SelectTrack { kind, id } => {
                let property = match kind {
                    TrackKind::Video => "vid",
                    TrackKind::Audio => "aid",
                    TrackKind::Subtitle => "sid",
                };
                // 用 `set` 命令而不是 set_property：vid/aid/sid 不接受 STRING
                // 格式的属性写入（报 "unsupported format for accessing property"），
                // 而 `set` 会按属性自身的类型去解析 "no" 和数字。id < 0 表示关闭。
                let value = if id < 0 {
                    "no".to_string()
                } else {
                    id.to_string()
                };
                api.run_command(handle, &["set", property, &value])?;
            }
            Command::AddSubtitle(path) => {
                api.run_command(handle, &["sub-add", &path, "select"])?;
            }
            Command::Cycle(property) => {
                api.run_command(handle, &["cycle", property])?;
            }
            Command::SetOption { name, value } => {
                // 同样走 `set`：属性类型各不相同（flag 认 "yes"/"no"，choice 认
                // 字符串，数值属性只认数字），命令层会按类型解析，比强塞 STRING 稳。
                api.run_command(handle, &["set", &name, &value])?;
            }
            Command::Screenshot(path) => {
                // 截图失败不影响播放，别走「播放出错」横幅：结果连同原因单独回给界面。
                match api.run_command(handle, &["screenshot-to-file", &path, "video"]) {
                    Ok(()) => shared.notify(PlayerEvent::Screenshot(Ok(PathBuf::from(path)))),
                    Err(error) => {
                        shared.notify(PlayerEvent::Screenshot(Err(format!("{error:#}"))));
                    }
                }
            }
        }
        Ok(())
    })();

    match result {
        Ok(()) => {}
        Err(error) => {
            log_warn!("命令执行失败: {error:#}");
            if report_failure {
                shared.notify(PlayerEvent::Error(format!("{error:#}")));
            }
        }
    }
    let _ = quit;
}

// ---------------------------------------------------------------------------
// 取帧线程
// ---------------------------------------------------------------------------

#[derive(Default)]
struct RenderSignal {
    dirty: AtomicBool,
    lock: Mutex<()>,
    condvar: Condvar,
}

impl RenderSignal {
    fn wake(&self) {
        self.dirty.store(true, Ordering::SeqCst);
        self.condvar.notify_all();
    }

    fn wait(&self, timeout: Duration) -> bool {
        if !self.dirty.load(Ordering::SeqCst) {
            let guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
            let _ = self
                .condvar
                .wait_timeout_while(guard, timeout, |_| !self.dirty.load(Ordering::SeqCst))
                .unwrap_or_else(|e| e.into_inner());
        }
        self.dirty.swap(false, Ordering::SeqCst)
    }
}

unsafe extern "C" fn on_render_update(data: *mut c_void) {
    if data.is_null() {
        return;
    }
    let signal = unsafe { &*(data as *const RenderSignal) };
    signal.wake();
}

fn render_loop(
    api: Arc<MpvApi>,
    handle: *mut ffi::MpvHandle,
    shared: Arc<Shared>,
    signal: Arc<RenderSignal>,
) {
    let mut context: *mut MpvRenderContext = std::ptr::null_mut();
    let api_type = ffi::RENDER_API_TYPE_SW.as_ptr() as *mut c_void;
    let mut params = [
        MpvRenderParam {
            kind: render_param::API_TYPE,
            data: api_type,
        },
        MpvRenderParam {
            kind: render_param::INVALID,
            data: std::ptr::null_mut(),
        },
    ];

    let rc = unsafe { (api.render_context_create)(&mut context, handle, params.as_mut_ptr()) };
    if rc < 0 {
        let message = api.error_text(rc);
        shared.notify(PlayerEvent::Error(format!("创建渲染上下文失败: {message}")));
        return;
    }

    unsafe {
        (api.render_context_set_update_callback)(
            context,
            Some(on_render_update),
            Arc::as_ptr(&signal) as *mut c_void,
        );
    }

    // 复用缓冲区：尺寸不变时不重新分配。mpv 文档要求 stride 与指针按 64 字节对齐，
    // 否则内部会走「整帧拷贝」的慢路径，所以这里自己做对齐，而不是直接扔一个 Vec。
    let mut buffer = FrameBuffer::default();
    let mut current = TargetSize::default();

    loop {
        if shared.shutdown.load(Ordering::SeqCst) {
            break;
        }

        // 等「有新帧」或「尺寸变了」。
        signal.wait(Duration::from_millis(40));

        if shared.shutdown.load(Ordering::SeqCst) {
            break;
        }

        let target = *shared.target.lock().unwrap_or_else(|e| e.into_inner());
        if target.width < 2 || target.height < 2 {
            continue;
        }
        if target != current {
            current = target;
            buffer.resize(current.width, current.height);
        }

        // mpv 告诉我们有帧才渲染；没有就继续等，避免空转。
        let flags = unsafe { (api.render_context_update)(context) };
        if flags & ffi::RENDER_UPDATE_FRAME == 0 {
            continue;
        }

        // 界面还没取走上一帧，就丢掉这一帧。播放器宁可掉帧也不能让内存堆积。
        if shared.frame_pending.swap(true, Ordering::SeqCst) {
            continue;
        }

        let rendered = frame::render_sw(&api, context, current.width, current.height, &mut buffer);
        if let Err(error) = rendered {
            log_warn!("渲染帧失败: {error:#}");
            shared.frame_pending.store(false, Ordering::SeqCst);
            continue;
        }

        let image = buffer.to_image();
        shared.frame_serial.fetch_add(1, Ordering::SeqCst);
        shared.notify(PlayerEvent::Frame(Arc::new(image)));
        unsafe { (api.render_context_report_swap)(context) };
    }

    unsafe {
        (api.render_context_set_update_callback)(context, None, std::ptr::null_mut());
        (api.render_context_free)(context);
    }
}

// ---------------------------------------------------------------------------
// 日志宏
// ---------------------------------------------------------------------------

impl Engine {
    /// 打开一个外部字幕文件。`path` 不存在时什么都不做（由调用方保证）。
    pub fn open_subtitle(&self, path: impl AsRef<Path>) {
        let path = path.as_ref().to_string_lossy().to_string();
        self.send(Command::AddSubtitle(path));
    }
}
