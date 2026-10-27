//! 进度条悬停预览：用**第二个** mpv 实例取「任意时间点的一张画面」。
//!
//! 为什么另起一个实例而不是在主实例上截图：主实例里是用户正在看的内容，
//! 为了预览去 seek 会把播放位置跳走。mpv 支持同进程多实例，这个实例不要声音、
//! 不要字幕、不建窗口，只做「打开 → 跳到指定时间 → 渲染一张小图」，
//! 开销远小于再解码一路视频。
//!
//! 线程模型：`mpv_render_context` 只能在创建它的线程里使用，所以句柄、渲染上下文
//! 和像素缓冲全部留在 `mpv-thumb` 线程上。界面侧只投递「时间点」，结果通过
//! `PlayerEvent::Thumbnail` 回到主线程——与主引擎完全一致的跨线程路径。

use crate::player::{
    engine::PlayerEvent,
    ffi::{self, MpvApi, MpvRenderContext, MpvRenderParam, SendPtr, event, render_param},
    frame::{self, FrameBuffer},
};
use anyhow::{Context as _, Result, anyhow};
use futures::channel::mpsc::UnboundedSender;
use gpui_kit::RenderImage;
use std::{
    ffi::{CString, c_void},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// 预览图的渲染宽度（物理像素）。
///
/// 界面按约 200 逻辑点显示，多出来的部分留给高分屏——否则 4K 屏上预览永远是糊的。
pub const WIDTH: u32 = 320;

/// 单次 seek 的等待上限。超过就放弃这一张：预览宁可慢一拍，也不能让线程
/// 卡在一个读不动的文件上（比如网络位置、刚被拔掉的移动硬盘）。
const SEEK_TIMEOUT: Duration = Duration::from_secs(3);
/// 打开文件的等待上限。
const LOAD_TIMEOUT: Duration = Duration::from_secs(4);

/// 按视频比例算出缩略图的渲染尺寸：宽度固定，高度跟着比例走。
///
/// 不统一成 16:9 是因为宽银幕片源会被压出两条黑边，4:3 的老片又会被拉伸，
/// 用户看的正是「这个位置长什么样」，比例失真就失去了参考价值。
pub fn render_size(aspect: f32) -> (u32, u32) {
    let aspect = if aspect.is_finite() && aspect > 0.2 {
        aspect
    } else {
        16.0 / 9.0
    };
    let height = (WIDTH as f32 / aspect).round().clamp(48.0, WIDTH as f32);
    (WIDTH, height as u32)
}

enum Request {
    /// 取 `path` 在 `time` 处的画面。
    Show {
        path: PathBuf,
        time: f64,
        height: u32,
    },
    /// 文件不用了：卸掉，别在后台占着文件句柄和解码器。
    Stop,
    Shutdown,
}

enum Work {
    Show {
        path: PathBuf,
        time: f64,
        height: u32,
    },
    Stop,
    Shutdown,
}

#[derive(Default)]
struct Signals {
    /// 界面每投递一次请求就自增。工作线程据此判断手上的活儿是否已经过期——
    /// 用户已经把鼠标挪走，这张图再送上去只会让预览倒退。
    generation: AtomicU64,
    /// 整个预览实例要销毁了，不必等 seek 超时。
    shutdown: AtomicBool,
}

/// 界面侧持有的句柄。丢掉它等于关掉整个预览实例。
pub struct Thumbnailer {
    requests: mpsc::Sender<Request>,
    signals: Arc<Signals>,
    thread: Option<JoinHandle<()>>,
}

impl Thumbnailer {
    /// 起一个预览实例。失败只影响悬停预览，不报给用户。
    pub fn spawn(api: Arc<MpvApi>, events: UnboundedSender<PlayerEvent>) -> Result<Self> {
        let (requests, receiver) = mpsc::channel();
        let signals = Arc::new(Signals::default());
        let thread_signals = Arc::clone(&signals);
        let thread = std::thread::Builder::new()
            .name("mpv-thumb".into())
            .spawn(move || worker(api, events, thread_signals, receiver))
            .context("启动缩略图线程")?;

        Ok(Self {
            requests,
            signals,
            thread: Some(thread),
        })
    }

    /// 请求 `path` 在 `time` 处的画面。`height` 由 [`render_size`] 按视频比例算好。
    pub fn show(&self, path: &Path, time: f64, height: u32) {
        // 先自增再投递：工作线程可能在收到请求前就检查了代号。
        self.signals.generation.fetch_add(1, Ordering::SeqCst);
        let _ = self.requests.send(Request::Show {
            path: path.to_path_buf(),
            time,
            height,
        });
    }

    pub fn stop(&self) {
        self.signals.generation.fetch_add(1, Ordering::SeqCst);
        let _ = self.requests.send(Request::Stop);
    }
}

impl Drop for Thumbnailer {
    fn drop(&mut self) {
        self.signals.shutdown.store(true, Ordering::SeqCst);
        let _ = self.requests.send(Request::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn worker(
    api: Arc<MpvApi>,
    events: UnboundedSender<PlayerEvent>,
    signals: Arc<Signals>,
    requests: mpsc::Receiver<Request>,
) {
    let mut worker = match Worker::new(api, events, signals) {
        Ok(worker) => worker,
        Err(error) => {
            // 预览是锦上添花的功能：起不来就退化成「没有悬停预览」，播放不受影响。
            log_warn!("进度条预览不可用: {error:#}");
            return;
        }
    };
    worker.run(&requests);
}

struct Worker {
    api: Arc<MpvApi>,
    handle: SendPtr<ffi::MpvHandle>,
    context: *mut MpvRenderContext,
    events: UnboundedSender<PlayerEvent>,
    signals: Arc<Signals>,
    buffer: FrameBuffer,
    /// 当前渲染尺寸。换文件、换比例时重新分配缓冲。
    size: (u32, u32),
    /// 已经装载的文件，避免每次悬停都重新打开一遍。
    media: Option<PathBuf>,
    /// 最近一次成功渲染的图与它对应的时间点。
    ///
    /// 留着它是为了「同一格」的重复请求能立刻答复：用户在同一秒里来回挪几个像素，
    /// 没必要每次都重新解码。**必须回包**——界面一次只挂一个请求，等不到回包
    /// 就不会发下一条，默默丢掉会让预览卡在旧位置。
    last: Option<(f64, Arc<RenderImage>)>,
}

impl Worker {
    fn new(
        api: Arc<MpvApi>,
        events: UnboundedSender<PlayerEvent>,
        signals: Arc<Signals>,
    ) -> Result<Self> {
        unsafe {
            let handle = (api.create)();
            if handle.is_null() {
                return Err(anyhow!("mpv_create 返回空指针"));
            }
            let handle = SendPtr(handle);

            // 这个实例只负责「把某一帧交出来」。逐项关掉不需要的能力，
            // 少一路音频解码、少一次字幕排版，预览就能更快出来。
            for (name, value) in [
                // 与主引擎同理：默认的 `gpu-next` 会自己建隐藏窗口，
                // render API 永远拿不到帧。`libmpv` 才会把帧交给渲染上下文。
                ("vo", "libmpv"),
                ("pause", "yes"),
                ("aid", "no"),
                ("sid", "no"),
                ("sub-auto", "no"),
                ("force-window", "no"),
                ("idle", "yes"),
                ("terminal", "no"),
                ("input-default-bindings", "no"),
                ("input-vo-keyboard", "no"),
                ("osd-level", "0"),
                // 预览必须落在鼠标指的那一格上，慢一点也要准。
                ("hr-seek", "yes"),
                ("cache", "auto"),
                // 只是一张缩略图，没必要像主实例那样缓 128MiB。
                ("demuxer-max-bytes", "32MiB"),
            ] {
                let Ok(cname) = CString::new(name) else {
                    continue;
                };
                let Ok(cvalue) = CString::new(value) else {
                    continue;
                };
                // 选项被拒绝不算失败：不同 mpv 版本认识的选项略有差异，
                // 少一个装饰性选项不该让预览整个不可用。
                (api.set_option_string)(handle.get(), cname.as_ptr(), cvalue.as_ptr());
            }

            let rc = (api.initialize)(handle.get());
            if rc < 0 {
                let message = api.error_text(rc);
                (api.terminate_destroy)(handle.get());
                return Err(anyhow!("缩略图实例初始化失败: {message}"));
            }

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
            let rc = (api.render_context_create)(&mut context, handle.get(), params.as_mut_ptr());
            if rc < 0 {
                let message = api.error_text(rc);
                (api.terminate_destroy)(handle.get());
                return Err(anyhow!("缩略图渲染上下文创建失败: {message}"));
            }

            Ok(Self {
                api,
                handle,
                context,
                events,
                signals,
                buffer: FrameBuffer::default(),
                size: (0, 0),
                media: None,
                last: None,
            })
        }
    }

    fn run(&mut self, requests: &mpsc::Receiver<Request>) {
        loop {
            let request = match requests.recv_timeout(Duration::from_millis(200)) {
                Ok(request) => request,
                // 空闲时也要醒一下，好及时看到 shutdown。
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };

            match latest(requests, request) {
                Work::Shutdown => break,
                Work::Stop => self.unload(),
                Work::Show { path, time, height } => {
                    let generation = self.signals.generation.load(Ordering::SeqCst);
                    self.show(&path, time, height, generation);
                }
            }
        }

        // 收尾顺序不能反：先放渲染上下文，再销毁实例。
        unsafe {
            (self.api.render_context_free)(self.context);
            (self.api.terminate_destroy)(self.handle.get());
        }
    }

    fn show(&mut self, path: &Path, time: f64, height: u32, generation: u64) {
        let wanted = (WIDTH, height.clamp(48, WIDTH));
        if self.size != wanted {
            self.buffer.resize(wanted.0, wanted.1);
            self.size = wanted;
            self.last = None;
        }

        if self.media.as_deref() != Some(path) && !self.load(path) {
            return;
        }

        // 同一格：把手上这张直接回过去，省一次解码。
        if let Some((shown, image)) = &self.last
            && (shown - time).abs() < 0.4
        {
            let _ = self.events.unbounded_send(PlayerEvent::Thumbnail {
                time,
                image: Arc::clone(image),
            });
            return;
        }

        if !self.seek(time, generation) {
            return;
        }

        let Some(image) = self.render(generation) else {
            return;
        };
        let image = Arc::new(image);
        self.last = Some((time, Arc::clone(&image)));
        let _ = self
            .events
            .unbounded_send(PlayerEvent::Thumbnail { time, image });
    }

    /// 换文件。等 `FILE_LOADED` 再走下一步：`loadfile` 是异步的，
    /// 紧跟其后的 `seek` 会撞上「文件还没打开」而直接失败。
    fn load(&mut self, path: &Path) -> bool {
        let url = path.to_string_lossy().to_string();
        if let Err(error) = self
            .api
            .run_command(self.handle.get(), &["loadfile", &url, "replace"])
        {
            log_debug!("缩略图实例打开失败: {error}");
            self.media = None;
            return false;
        }
        self.media = Some(path.to_path_buf());
        self.last = None;

        // 这里刻意**不看**「请求是否过期」：本地文件通常几十毫秒就绪，
        // 而中途放弃会留下「半加载」的状态——`media` 已经记上了，下一个请求
        // 便跳过这里直接 seek，撞上的正是「文件还没打开」。等它加载完更稳。
        let deadline = Instant::now() + LOAD_TIMEOUT;
        while Instant::now() < deadline {
            if self.signals.shutdown.load(Ordering::SeqCst) {
                return false;
            }
            match self.next_event(0.02) {
                Some(event::FILE_LOADED) => return true,
                // 打不开（文件不存在、格式不支持）：安静放弃，别把错误横幅
                // 甩到正在播放的界面上。
                Some(event::END_FILE) => {
                    self.media = None;
                    return false;
                }
                _ => {}
            }
        }
        self.media = None;
        false
    }

    /// 跳转并等到目标位置的帧。
    fn seek(&mut self, time: f64, generation: u64) -> bool {
        // 跳转前先把「有新帧」这个标志消耗掉。
        //
        // 打开文件后 mpv 已经把第一帧摆在那儿了；不消耗的话，seek 之后第一次
        // `update()` 看到的还是那一帧（片头），预览会永远错一格——而且错得很隐蔽：
        // `time-pos` 已经是目标值，只有画面不对。代价是一次 320×180 的空渲染。
        self.discard_frame();

        let target = format!("{time:.3}");
        if let Err(error) = self
            .api
            .run_command(self.handle.get(), &["seek", &target, "absolute+exact"])
        {
            log_debug!("缩略图实例跳转失败: {error}");
            return false;
        }

        let deadline = Instant::now() + SEEK_TIMEOUT;
        while Instant::now() < deadline {
            if self.stale(generation) {
                return false;
            }
            // 位置对上了、并且 mpv 报告有新帧，才可以把这块缓冲当成这一格。
            // 少了位置检查，刚跳转时渲染出来的是上一格的画面——预览会一直
            // 落后指针一拍，用户以为预览不准。
            if self.at_target(time) && self.has_frame() {
                return true;
            }
            // 事件队列要有人取，否则 mpv 会在内部堆积到上限。
            self.next_event(0.0);
            std::thread::sleep(Duration::from_millis(3));
        }
        false
    }

    fn at_target(&self, time: f64) -> bool {
        self.api
            .property_f64(self.handle.get(), "time-pos")
            .is_some_and(|position| (position - time).abs() <= 0.5)
    }

    fn has_frame(&self) -> bool {
        unsafe { (self.api.render_context_update)(self.context) & ffi::RENDER_UPDATE_FRAME != 0 }
    }

    /// 把已经挂着的帧画掉并「换页」，让 `has_frame` 重新变回假。
    fn discard_frame(&mut self) {
        if !self.has_frame() {
            return;
        }
        let (width, height) = self.size;
        if frame::render_sw(&self.api, self.context, width, height, &mut self.buffer).is_ok() {
            unsafe { (self.api.render_context_report_swap)(self.context) };
        }
    }

    fn render(&mut self, generation: u64) -> Option<RenderImage> {
        // 等的这几毫秒里用户可能已经指向别处，画出来也没人要了。
        if self.stale(generation) {
            return None;
        }
        let (width, height) = self.size;
        let rendered = frame::render_sw(&self.api, self.context, width, height, &mut self.buffer);
        unsafe { (self.api.render_context_report_swap)(self.context) };
        match rendered {
            Ok(()) => Some(self.buffer.to_image()),
            Err(error) => {
                log_debug!("缩略图渲染失败: {error:#}");
                None
            }
        }
    }

    fn unload(&mut self) {
        if self.media.take().is_some() {
            let _ = self.api.run_command(self.handle.get(), &["stop"]);
        }
        self.last = None;
    }

    /// 手上的活儿是否已经过期（界面换了位置，或者整个实例要销毁）。
    fn stale(&self, generation: u64) -> bool {
        self.signals.shutdown.load(Ordering::SeqCst)
            || self.signals.generation.load(Ordering::SeqCst) != generation
    }

    /// 取一个事件；`None` 表示这段时间内没有事件。
    fn next_event(&self, timeout: f64) -> Option<i32> {
        let event = unsafe { (self.api.wait_event)(self.handle.get(), timeout) };
        if event.is_null() {
            return None;
        }
        Some(unsafe { (*event).event_id })
    }
}

/// 把队列里已经排着的请求一次性取到最新的一条。
///
/// 鼠标划过进度条时请求是一串一串来的：逐条 seek 只会让预览永远追着指针跑，
/// 而且越追越远。这里只保留最后一条，中间那些位置用户早就看过去了。
fn latest(requests: &mpsc::Receiver<Request>, first: Request) -> Work {
    let mut work = match first {
        Request::Shutdown => return Work::Shutdown,
        Request::Stop => Work::Stop,
        Request::Show { path, time, height } => Work::Show { path, time, height },
    };
    while let Ok(next) = requests.try_recv() {
        work = match next {
            Request::Shutdown => return Work::Shutdown,
            Request::Stop => Work::Stop,
            Request::Show { path, time, height } => Work::Show { path, time, height },
        };
    }
    work
}
