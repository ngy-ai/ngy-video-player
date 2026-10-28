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

// 引擎按职责拆分：共享状态与句柄留在本文件，线程逻辑分散到子模块。
mod dispatch;
mod event_loop;
mod render_loop;
mod state;

pub use self::state::*;

use self::dispatch::dispatch;
use self::event_loop::event_loop;
use self::render_loop::render_loop;

struct Shared {
    state: Mutex<Snapshot>,
    events: UnboundedSender<PlayerEvent>,
    shutdown: AtomicBool,
    /// 渲染目标：画布尺寸（物理像素）+ 拷帧时的旋转。界面每次布局确定后写一次。
    target: Mutex<RenderTarget>,
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
    /// 界面要求「就算没有新帧也重画一次」。画面比例、镜像这类设置不改变渲染
    /// 目标尺寸，暂停时 mpv 也不会主动送帧，只能由界面点一下。
    redraw: AtomicBool,
}

/// 渲染目标：mpv 往多大的画布上画，以及这帧画完要不要转。
///
/// 旋转没走 mpv 的 `video-rotate`：libmpv 的软件渲染路径（`MPV_RENDER_API_TYPE_SW`）
/// 拿不到 VO 的 ROTATE90 能力位，那个属性只会改源矩形的裁切，像素一个都不转，
/// 画面反而被裁烂（实测）。所以改成在拷帧时自己转——一次 memcpy 量级的操作，
/// 换来的是三个平台、软硬解都一样的结果。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RenderTarget {
    width: u32,
    height: u32,
    /// 顺时针角度，只取 0 / 90 / 180 / 270。
    rotation: u16,
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
    render_target: Mutex<RenderTarget>,
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
    pub fn spawn(events: UnboundedSender<PlayerEvent>, options: &EngineOptions) -> Result<Self> {
        let api = MpvApi::load()?;
        let shared = Arc::new(Shared {
            state: Mutex::new(Snapshot::default()),
            events,
            shutdown: AtomicBool::new(false),
            target: Mutex::new(RenderTarget::default()),
            frame_serial: AtomicU64::new(0),
            needs_frames: AtomicBool::new(false),
            frame_pending: AtomicBool::new(false),
            config_serial: AtomicU32::new(0),
            redraw: AtomicBool::new(false),
        });

        let handle = unsafe { (api.create)() };
        if handle.is_null() {
            return Err(anyhow!("mpv_create 返回空指针"));
        }
        let handle = SendPtr(handle);

        unsafe {
            Self::configure(&api, handle.get(), options).context("初始化 libmpv 配置")?;

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
            render_target: Mutex::new(RenderTarget::default()),
            thumbnail: Mutex::new(None),
            thumbnail_failed: AtomicBool::new(false),
        })
    }

    /// mpv 侧的初始配置。这里的每一项都在解决一个具体问题，见各自注释。
    ///
    /// 选项被拒绝只记一条 warn：不同 mpv 版本认识的选项不完全一样，
    /// 一个装饰性选项（比如字幕字号）不该让整个播放器起不来。
    unsafe fn configure(
        api: &MpvApi,
        handle: *mut ffi::MpvHandle,
        options: &EngineOptions,
    ) -> Result<()> {
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
            // 解码方式与用哪块显卡是用户的偏好，必须在 initialize 之前落成选项：
            // 这两项都在解码器 / VO 建立时才读，运行期 `set` 只能改下一次。
            opt(api, handle, "hwdec", &options.hwdec);
            if let Some(adapter) = &options.d3d11_adapter {
                opt(api, handle, "d3d11-adapter", adapter);
            }
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

    /// 界面在布局确定后调用：告诉取帧线程应该按多大尺寸渲染、要不要转。
    ///
    /// 尺寸的比例**必须与画面设置想要的口径一致**，mpv 才会在这个画布里按
    /// 视频自身比例排黑边；调用方负责按「不超过视频原始分辨率」等比缩放，
    /// 这里只做范围夹取。
    pub fn set_render_target(&self, width: u32, height: u32, rotation: u16) {
        let next = RenderTarget {
            width: width.clamp(2, 7680),
            height: height.clamp(2, 4320),
            rotation: rotation % 360,
        };
        let mut current = self.render_target.lock().unwrap_or_else(|e| e.into_inner());
        if *current != next {
            *current = next;
            *self.shared.target.lock().unwrap_or_else(|e| e.into_inner()) = next;
            // 旋转变了但尺寸没变时也必须重画一次，否则暂停中的画面会一直保持旧角度。
            self.signal.wake();
        }
    }

    pub fn target_size(&self) -> (u32, u32) {
        let target = *self.render_target.lock().unwrap_or_else(|e| e.into_inner());
        (target.width, target.height)
    }

    /// 要求取帧线程立刻重画一帧，即使 mpv 没有新画面。
    ///
    /// 画面比例、镜像这类设置只改变「同一帧怎么画」，渲染目标的尺寸可以不变；
    /// 暂停时 mpv 也就不会送新帧，不点这一下界面上看到的还是旧画面。
    pub fn request_redraw(&self) {
        self.shared.redraw.store(true, Ordering::SeqCst);
        self.signal.wake();
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

// ---------------------------------------------------------------------------
// 外部字幕
// ---------------------------------------------------------------------------

impl Engine {
    /// 打开一个外部字幕文件。`path` 不存在时什么都不做（由调用方保证）。
    pub fn open_subtitle(&self, path: impl AsRef<Path>) {
        let path = path.as_ref().to_string_lossy().to_string();
        self.send(Command::AddSubtitle(path));
    }
}
