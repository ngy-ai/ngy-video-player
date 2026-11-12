//! 界面侧持有的合成器句柄：投递请求、下发画布尺寸、取快照。

use crate::player::{engine::PlayerEvent, ffi::MpvApi};
use anyhow::{Context as _, Result};
use futures::channel::mpsc::UnboundedSender;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc,
    },
    thread::JoinHandle,
};

use super::{SNAPSHOT_TIMEOUT, layer::LayerSpec, worker::Worker};

/// 合成好的一帧（紧密 BGRA），给截图 / 示波器这类「要真的这张图」的地方用。
pub(crate) struct Snapshot {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

pub(super) enum Request {
    /// 换一整套图层。`seek` 为假表示只更新透明度与播放状态（播放中的动画），
    /// 不去动位置——每帧都补 seek 会让声音和画面一起打嗝。
    Present {
        layers: Vec<LayerSpec>,
        seek: bool,
    },
    /// 把当前合成结果拷一份回传。
    Snapshot(mpsc::Sender<Option<Snapshot>>),
    /// 两路都卸掉，留着实例等下一次。
    Stop,
    Shutdown,
}

/// 画布尺寸：由界面按「视频区域该占多大」下发，窗口一变就跟着变。
#[derive(Default)]
pub(super) struct Target {
    width: AtomicU32,
    height: AtomicU32,
}

impl Target {
    pub(super) fn get(&self) -> (u32, u32) {
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

    /// 取一份当前合成结果（截图、示波器用）。
    ///
    /// 合成线程空闲时每 4ms 醒一次，回包基本是「一次 2MB 拷贝」的时间，所以在界面
    /// 线程上同步等是可以接受的；等待上限只是兜底——合成器不响应时返回 `None`，
    /// 调用方退回自己那条老路。
    pub(crate) fn snapshot(&self) -> Option<Snapshot> {
        let (sender, receiver) = mpsc::channel();
        if self.requests.send(Request::Snapshot(sender)).is_err() {
            return None;
        }
        receiver.recv_timeout(SNAPSHOT_TIMEOUT).ok().flatten()
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
