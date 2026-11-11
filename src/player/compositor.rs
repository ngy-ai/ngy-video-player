//! 多轨软件合成预览（M10-T01）：每条视频轨一个 mpv，SW 渲染到内存后由 CPU 叠起来。
//!
//! 为什么要在自己这边合成：预览的主实例一次只解一路画面（`player::engine`），
//! 多轨叠加、画中画这类「改一下要立刻看到」的东西它给不了。ADR 0008 定的路线是
//! 「多实例 + 合成」，第一步先做 CPU 版（架构 A），GPU 版（`M10-T02`）之后再来。
//!
//! 三段约定：
//!
//! * **每一路都按同一个画布尺寸渲染**。缩放与黑边交给 mpv（它自己按
//!   `keepaspect` 排），于是合成只剩逐像素的 alpha 混合，不用二次缩放——
//!   1080p 下每帧省掉一次全画面的重采样。
//! * **合成结果仍然走 `PlayerEvent`**。主线程拿到的还是一张 `RenderImage`，
//!   `incoming` / `swap_frame` / 画面排版一行都不用改。
//! * **渲染上下文只在创建它的线程上用**。句柄、上下文、像素缓冲全部留在
//!   `mpv-composite` 线程，界面侧只投递请求。
//!
//! 图层顺序自下而上（`LayerSpec` 列表第 0 个是底），与 `timeline::preview::layers`
//! 的输出一致。一层都没就绪时什么都不发——界面留着上一帧，比闪一下黑好。

use crate::player::{
    engine::PlayerEvent,
    ffi::{self, MpvApi, MpvRenderContext, MpvRenderParam, SendPtr, event, render_param},
    frame::{self, FrameBuffer},
};
use anyhow::{Context as _, Result, anyhow};
use futures::channel::mpsc::UnboundedSender;
use gpui_kit::RenderImage;
use image::{Frame, ImageBuffer, Rgba};
use std::{
    ffi::{CString, c_void},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// 同时参与合成的视频轨上限。
///
/// ADR 0008 给的是 4 路，第一步只做两路：CPU 合成 1080p 两路已经是 24fps 的边，
/// 四路得等 GPU 后端（`M10-T02`）。超出的轨不参与合成，由界面提示一句。
pub(crate) const MAX_LAYERS: usize = 2;

/// 打开文件的等待上限（与缩略图实例同一档）。
const LOAD_TIMEOUT: Duration = Duration::from_secs(4);
/// 单次 seek 的等待上限。
const SEEK_TIMEOUT: Duration = Duration::from_secs(3);
/// 位置差多少才值得补一次 seek。
const SEEK_TOLERANCE: f64 = 0.08;
/// 空闲时醒来的间隔：既要不影响播放的跟手程度，又不至于空转烧 CPU。
const TICK: Duration = Duration::from_millis(4);

/// 一层画面要放什么。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LayerSpec {
    pub path: PathBuf,
    /// 素材内的时刻（秒）。
    pub source: f64,
    /// 这一层要不要跟着时间走。定格段与暂停时为假。
    pub playing: bool,
    /// 0 = 完全透明。
    pub opacity: f32,
}

enum Request {
    /// 换一整套图层。`seek` 为假表示只更新透明度与播放状态（播放中的动画），
    /// 不去动位置——每帧都补 seek 会让声音和画面一起打嗝。
    Present {
        layers: Vec<LayerSpec>,
        seek: bool,
    },
    /// 两路都卸掉，留着实例等下一次。
    Stop,
    Shutdown,
}

/// 画布尺寸：由界面按「视频区域该占多大」下发，窗口一变就跟着变。
#[derive(Default)]
struct Target {
    width: AtomicU32,
    height: AtomicU32,
}

impl Target {
    fn get(&self) -> (u32, u32) {
        (
            self.width.load(Ordering::SeqCst).max(2),
            self.height.load(Ordering::SeqCst).max(2),
        )
    }
}

/// 界面侧持有的句柄。丢掉它等于关掉整个合成实例。
pub(crate) struct Compositor {
    requests: mpsc::Sender<Request>,
    target: Arc<Target>,
    /// 界面上还有一帧没画：这期间合成出来的帧直接丢，免得队列越排越长。
    pending: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Compositor {
    /// 起一个合成实例。失败只影响多轨预览，单路播放不受影响。
    pub(crate) fn spawn(api: Arc<MpvApi>, events: UnboundedSender<PlayerEvent>) -> Result<Self> {
        let (requests, receiver) = mpsc::channel();
        let target = Arc::new(Target::default());
        let pending = Arc::new(AtomicBool::new(false));
        let thread_target = Arc::clone(&target);
        let thread_pending = Arc::clone(&pending);
        let thread = std::thread::Builder::new()
            .name("mpv-composite".into())
            .spawn(move || {
                match Worker::new(api, events, thread_target, thread_pending) {
                    Ok(mut worker) => worker.run(&receiver),
                    // 起不来（显存 / 渲染上下文不给）就退化成单路，界面会提示。
                    Err(error) => log_warn!("多轨合成预览不可用: {error:#}"),
                }
            })
            .context("启动合成预览线程")?;

        Ok(Self {
            requests,
            target,
            pending,
            thread: Some(thread),
        })
    }

    /// 下发一整套图层（自下而上）。
    pub(crate) fn present(&self, layers: Vec<LayerSpec>, seek: bool) {
        let _ = self.requests.send(Request::Present { layers, seek });
    }

    pub(crate) fn stop(&self) {
        let _ = self.requests.send(Request::Stop);
    }

    pub(crate) fn set_target(&self, width: u32, height: u32) {
        self.target.width.store(width.max(2), Ordering::SeqCst);
        self.target.height.store(height.max(2), Ordering::SeqCst);
    }

    /// 界面把帧取走了：可以再产下一帧。
    pub(crate) fn frame_consumed(&self) {
        self.pending.store(false, Ordering::SeqCst);
    }
}

impl Drop for Compositor {
    fn drop(&mut self) {
        let _ = self.requests.send(Request::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// 一路 mpv：句柄、渲染上下文、像素缓冲。
struct Layer {
    handle: SendPtr<ffi::MpvHandle>,
    context: *mut MpvRenderContext,
    buffer: FrameBuffer,
    media: Option<PathBuf>,
    source: f64,
    opacity: f32,
    /// 缓冲里是不是这一层的有效画面。换文件、改尺寸时置假。
    ready: bool,
}

impl Layer {
    fn new(api: &MpvApi) -> Result<Self> {
        unsafe {
            let handle = (api.create)();
            if handle.is_null() {
                return Err(anyhow!("mpv_create 返回空指针"));
            }
            let handle = SendPtr(handle);

            // 与缩略图实例同一套思路：只要画面，不要声音 / 字幕 / 窗口。
            for (name, value) in [
                // `gpu-next` 会自己建隐藏窗口，render API 永远拿不到帧。
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
                // 合成靠像素对齐，seek 必须落在准确的帧上。
                ("hr-seek", "yes"),
                // 画布上没被画面盖住的地方留纯黑，与导出侧 `pad` 的黑边一致。
                ("background-color", "#000000"),
                ("cache", "auto"),
                ("demuxer-max-bytes", "32MiB"),
            ] {
                let Ok(cname) = CString::new(name) else {
                    continue;
                };
                let Ok(cvalue) = CString::new(value) else {
                    continue;
                };
                (api.set_option_string)(handle.get(), cname.as_ptr(), cvalue.as_ptr());
            }

            let rc = (api.initialize)(handle.get());
            if rc < 0 {
                let message = api.error_text(rc);
                (api.terminate_destroy)(handle.get());
                return Err(anyhow!("合成实例初始化失败: {message}"));
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
                return Err(anyhow!("合成渲染上下文创建失败: {message}"));
            }

            Ok(Self {
                handle,
                context,
                buffer: FrameBuffer::default(),
                media: None,
                source: 0.0,
                opacity: 1.0,
                ready: false,
            })
        }
    }

    /// 换文件。等 `FILE_LOADED` 再往下走：`loadfile` 是异步的。
    fn load(&mut self, api: &MpvApi, path: &Path) -> bool {
        let url = path.to_string_lossy().to_string();
        if let Err(error) = api.run_command(self.handle.get(), &["loadfile", &url, "replace"]) {
            log_debug!("合成实例打开失败: {error}");
            self.media = None;
            self.ready = false;
            return false;
        }
        self.media = Some(path.to_path_buf());
        self.ready = false;
        self.buffer.clear();

        let deadline = Instant::now() + LOAD_TIMEOUT;
        while Instant::now() < deadline {
            match self.next_event(api, 0.02) {
                Some(event::FILE_LOADED) => return true,
                Some(event::END_FILE) => {
                    // 打不开（文件没了 / 格式不支持）：这一层空着，别把错误甩给用户，
                    // 但日志里要留一条——少一层画面时这是唯一线索。
                    log_warn!("合成预览这一层打不开: {}", path.display());
                    self.media = None;
                    return false;
                }
                _ => {}
            }
        }
        log_warn!("合成预览这一层加载超时: {}", path.display());
        self.media = None;
        false
    }

    /// 跳到素材内的 `time` 秒，并等到那一帧真的解出来。
    fn seek(&mut self, api: &MpvApi, time: f64, size: (u32, u32)) -> bool {
        // 跳转前先把「有新帧」这个标志消耗掉，否则第一次 update 看到的还是旧帧。
        self.discard_frame(api, size);

        let target = format!("{time:.3}");
        if let Err(error) = api.run_command(self.handle.get(), &["seek", &target, "absolute+exact"])
        {
            log_debug!("合成实例跳转失败: {error}");
            return false;
        }

        let deadline = Instant::now() + SEEK_TIMEOUT;
        while Instant::now() < deadline {
            if self.at_target(api, time) && self.has_frame(api) {
                self.source = time;
                // 当场画掉这一帧：留到下一层处理完再回来，mpv 可能已经把它丢了
                // （多路依次 seek 时中间隔着上百毫秒），这一层就会被判成「没有画面」。
                return self.render(api, size.0, size.1);
            }
            // 事件队列要有人取，否则 mpv 内部会堆到上限。
            self.next_event(api, 0.0);
            std::thread::sleep(Duration::from_millis(3));
        }
        let position = api
            .property_f64(self.handle.get(), "time-pos")
            .unwrap_or(f64::NAN);
        log_warn!(
            "合成预览跳转超时: 目标 {time:.3}s，当前 {position:.3}s，有帧={}",
            self.has_frame(api)
        );
        false
    }

    /// 位置对上没有。
    ///
    /// 公差必须**紧**：刚 `loadfile` 完时位置还停在 0，而目标可能就在 0.5 秒以内；
    /// 用宽公差会当场认为「到位了」，随后 mpv 因为 seek 丢掉手里那一帧，
    /// 于是这一层被判成「没有画面」——表现就是上层单独显示、底层一直黑着。
    fn at_target(&self, api: &MpvApi, time: f64) -> bool {
        api.property_f64(self.handle.get(), "time-pos")
            .is_some_and(|position| (position - time).abs() <= SEEK_TOLERANCE)
    }

    /// 已经停在目标位置附近：这时候再补一次 seek 只是白解一遍。
    fn close_to(&self, api: &MpvApi, time: f64) -> bool {
        api.property_f64(self.handle.get(), "time-pos")
            .is_some_and(|position| (position - time).abs() <= SEEK_TOLERANCE)
    }

    fn has_frame(&self, api: &MpvApi) -> bool {
        unsafe { (api.render_context_update)(self.context) & ffi::RENDER_UPDATE_FRAME != 0 }
    }

    /// 把已经挂着的帧画掉并「换页」，让 `has_frame` 重新变回假。
    fn discard_frame(&mut self, api: &MpvApi, size: (u32, u32)) {
        if !self.has_frame(api) {
            return;
        }
        if frame::render_sw(api, self.context, size.0, size.1, &mut self.buffer).is_ok() {
            unsafe { (api.render_context_report_swap)(self.context) };
        }
    }

    /// 把当前帧渲染进这一层的缓冲。返回是否真的画了。
    fn render(&mut self, api: &MpvApi, width: u32, height: u32) -> bool {
        let rendered = frame::render_sw(api, self.context, width, height, &mut self.buffer);
        unsafe { (api.render_context_report_swap)(self.context) };
        match rendered {
            Ok(()) => {
                self.ready = true;
                true
            }
            Err(error) => {
                log_debug!("合成实例渲染失败: {error:#}");
                false
            }
        }
    }

    fn set_pause(&self, api: &MpvApi, paused: bool) {
        let value = if paused { "yes" } else { "no" };
        if let Err(error) = api.run_command(self.handle.get(), &["set", "pause", value]) {
            log_debug!("合成实例暂停失败: {error}");
        }
    }

    fn unload(&mut self, api: &MpvApi) {
        if self.media.take().is_some() {
            let _ = api.run_command(self.handle.get(), &["stop"]);
        }
        self.ready = false;
    }

    fn next_event(&self, api: &MpvApi, timeout: f64) -> Option<i32> {
        let event = unsafe { (api.wait_event)(self.handle.get(), timeout) };
        if event.is_null() {
            return None;
        }
        Some(unsafe { (*event).event_id })
    }

    fn destroy(&mut self, api: &MpvApi) {
        // 收尾顺序不能反：先放渲染上下文，再销毁实例。
        unsafe {
            (api.render_context_free)(self.context);
            (api.terminate_destroy)(self.handle.get());
        }
    }
}

struct Worker {
    api: Arc<MpvApi>,
    events: UnboundedSender<PlayerEvent>,
    target: Arc<Target>,
    pending: Arc<AtomicBool>,
    layers: Vec<Layer>,
    /// 当前画布尺寸。与目标不一致时重新分配缓冲并补一次 seek（暂停时也得重画）。
    size: (u32, u32),
}

impl Worker {
    fn new(
        api: Arc<MpvApi>,
        events: UnboundedSender<PlayerEvent>,
        target: Arc<Target>,
        pending: Arc<AtomicBool>,
    ) -> Result<Self> {
        let mut layers = Vec::with_capacity(MAX_LAYERS);
        for index in 0..MAX_LAYERS {
            layers.push(Layer::new(&api).with_context(|| format!("第 {} 路合成实例", index + 1))?);
        }
        Ok(Self {
            api,
            events,
            target,
            pending,
            layers,
            size: (0, 0),
        })
    }

    fn run(&mut self, requests: &mpsc::Receiver<Request>) {
        loop {
            match requests.recv_timeout(TICK) {
                Ok(Request::Shutdown) => break,
                Ok(Request::Stop) => {
                    for layer in &mut self.layers {
                        layer.unload(&self.api);
                    }
                }
                Ok(Request::Present { layers, seek }) => {
                    // 拖动播放头时请求是一串一串来的：只做最后一条，
                    // 中间那些位置用户早就划过去了。
                    let mut work = (layers, seek);
                    let mut shutdown = false;
                    loop {
                        match requests.try_recv() {
                            Ok(Request::Shutdown) => {
                                shutdown = true;
                                break;
                            }
                            Ok(Request::Stop) => {
                                for layer in &mut self.layers {
                                    layer.unload(&self.api);
                                }
                            }
                            Ok(Request::Present { layers, seek }) => work = (layers, seek),
                            Err(_) => break,
                        }
                    }
                    if shutdown {
                        break;
                    }
                    self.present(work.0, work.1);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            self.pump();
        }
        self.destroy();
    }

    fn destroy(&mut self) {
        for layer in &mut self.layers {
            layer.destroy(&self.api);
        }
    }

    /// 按目标尺寸重新分配缓冲。返回是否真的改了尺寸。
    fn resize(&mut self) -> bool {
        let wanted = self.target.get();
        if self.size == wanted {
            return false;
        }
        log_debug!("合成画布换尺寸 {:?} -> {wanted:?}", self.size);
        self.size = wanted;
        for layer in &mut self.layers {
            layer.buffer.resize(wanted.0, wanted.1);
            layer.buffer.clear();
            layer.ready = false;
        }
        true
    }

    fn present(&mut self, specs: Vec<LayerSpec>, seek: bool) {
        self.resize();
        // 多出来的层卸掉：上层被删 / 关掉了。
        for layer in self.layers.iter_mut().skip(specs.len()) {
            layer.unload(&self.api);
            layer.opacity = 0.0;
        }

        let size = self.size;
        for (layer, spec) in self.layers.iter_mut().zip(&specs) {
            layer.opacity = if spec.opacity.is_finite() {
                spec.opacity.clamp(0.0, 1.0)
            } else {
                1.0
            };
            let switched = layer.media.as_deref() != Some(spec.path.as_path());
            log_debug!(
                "合成层 {} @ {:.3}s opacity={:.2} seek={} switched={switched}",
                spec.path.display(),
                spec.source,
                layer.opacity,
                seek || switched
            );
            if switched && !layer.load(&self.api, &spec.path) {
                continue;
            }
            // 只更新透明度时不动位置：播放中每帧补 seek 会把画面和声音一起卡住。
            // 已经停在目标位置上也不再补 seek：拖播放头时相邻两帧往往落在同一处。
            let needs_seek =
                (seek || switched) && (!layer.ready || !layer.close_to(&self.api, spec.source));
            if needs_seek && !layer.seek(&self.api, spec.source, size) {
                log_debug!("合成层 {} 放弃: seek 失败", spec.path.display());
                continue;
            }
            layer.source = spec.source;
            layer.set_pause(&self.api, !spec.playing);
        }

        // 摆好就立刻出一帧，别等下一次新帧（暂停时永远等不到）。
        let mut changed = false;
        let mut ready = 0;
        for layer in self.layers.iter_mut() {
            if layer.media.is_none() {
                continue;
            }
            if layer.has_frame(&self.api) && layer.render(&self.api, self.size.0, self.size.1) {
                changed = true;
            }
            if layer.ready {
                ready += 1;
            }
        }
        log_debug!(
            "合成 {}x{}：{} 层就绪，seek={seek}",
            self.size.0,
            self.size.1,
            ready
        );
        if changed {
            self.composite_and_send();
        }
    }

    /// 播放中：两边有新帧就各画一次，然后合起来。
    fn pump(&mut self) {
        let resized = self.resize();
        if resized {
            // 尺寸变了，mpv 手里那帧已经作废：回到原位置重画一次。
            let size = self.size;
            for layer in &mut self.layers {
                if layer.media.is_some() {
                    let source = layer.source;
                    layer.seek(&self.api, source, size);
                    layer.render(&self.api, size.0, size.1);
                }
            }
            self.composite_and_send();
            return;
        }

        let mut changed = false;
        for layer in &mut self.layers {
            if layer.media.is_none() {
                continue;
            }
            // 事件队列要有人取，否则 mpv 内部会堆到上限。
            layer.next_event(&self.api, 0.0);
            if layer.has_frame(&self.api) && layer.render(&self.api, self.size.0, self.size.1) {
                changed = true;
            }
        }
        if changed {
            self.composite_and_send();
        }
    }

    /// 把就绪的层自下而上叠成一张图，交给界面。
    fn composite_and_send(&mut self) {
        let (width, height) = self.size;
        if width < 2 || height < 2 {
            return;
        }
        let ready: Vec<usize> = self
            .layers
            .iter()
            .enumerate()
            .filter(|(_, layer)| layer.ready && layer.media.is_some())
            .map(|(index, _)| index)
            .collect();
        if ready.is_empty() {
            return;
        }
        // 界面还没画完上一帧：这一帧丢掉，与主引擎的背压同一套语义。
        if self.pending.load(Ordering::SeqCst) {
            return;
        }

        let image = if ready.len() == 1 {
            // 只有一层有画面（另一路还在加载 / 打不开）：不必过一遍画布。
            self.layers[ready[0]].buffer.to_image(0)
        } else {
            let mut canvas = vec![0u8; width as usize * height as usize * 4];
            for (order, &index) in ready.iter().enumerate() {
                // 最底下那层是不透明底：拿它开画布，不能再按透明度压暗一次。
                let opacity = if order == 0 {
                    1.0
                } else {
                    self.layers[index].opacity
                };
                self.layers[index].buffer.blend_into(&mut canvas, opacity);
            }
            let buffer: ImageBuffer<Rgba<u8>, Vec<u8>> =
                ImageBuffer::from_raw(width, height, canvas).expect("画布尺寸由我们自己分配");
            RenderImage::new([Frame::new(buffer)])
        };

        self.pending.store(true, Ordering::SeqCst);
        let _ = self.events.unbounded_send(PlayerEvent::Composite {
            image: Arc::new(image),
        });
    }
}
