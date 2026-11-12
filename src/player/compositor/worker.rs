//! 合成线程本体：收请求、驱动各路 seek / 渲染，把叠好的一帧发回界面。

use crate::player::{engine::PlayerEvent, ffi::MpvApi};
use anyhow::{Context as _, Result};
use futures::channel::mpsc::UnboundedSender;
use gpui_kit::RenderImage;
use image::{Frame, ImageBuffer, Rgba};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use super::{
    MAX_LAYERS, TICK,
    handle::{Request, Snapshot, Target},
    layer::{Layer, LayerSpec},
};

pub(super) struct Worker {
    api: Arc<MpvApi>,
    events: UnboundedSender<PlayerEvent>,
    target: Arc<Target>,
    pending: Arc<AtomicBool>,
    layers: Vec<Layer>,
    /// 当前画布尺寸。与目标不一致时重新分配缓冲并补一次 seek（暂停时也得重画）。
    size: (u32, u32),
}

impl Worker {
    pub(super) fn new(
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

    pub(super) fn run(&mut self, requests: &mpsc::Receiver<Request>) {
        loop {
            match requests.recv_timeout(TICK) {
                Ok(Request::Shutdown) => break,
                Ok(Request::Stop) => {
                    for layer in &mut self.layers {
                        layer.unload(&self.api);
                    }
                }
                Ok(Request::Snapshot(reply)) => {
                    let _ = reply.send(self.snapshot());
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
                            Ok(Request::Snapshot(reply)) => {
                                let _ = reply.send(self.snapshot());
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
                "合成层 {} @ {:.3}s opacity={:.2} rate={:.2} seek={} switched={switched}",
                spec.path.display(),
                spec.source,
                layer.opacity,
                spec.rate,
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
            layer.set_speed(&self.api, spec.rate);
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

    /// 把就绪的层自下而上叠成一块紧密 BGRA。一层就绪时直接拷它，不必过画布。
    ///
    /// 上屏与「截图 / 示波器」共用这一条路径：两边拿到的必须是同一张图，否则
    /// 用户截出来的和看到的不一样。
    fn canvas(&self) -> Option<(u32, u32, Vec<u8>)> {
        let (width, height) = self.size;
        if width < 2 || height < 2 {
            return None;
        }
        let ready: Vec<usize> = self
            .layers
            .iter()
            .enumerate()
            .filter(|(_, layer)| layer.ready && layer.media.is_some())
            .map(|(index, _)| index)
            .collect();
        let first = *ready.first()?;
        if ready.len() == 1 {
            // 只有一层有画面（另一路还在加载 / 打不开）：不必过一遍画布。
            return Some((width, height, self.layers[first].buffer.to_bgra()));
        }
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
        Some((width, height, canvas))
    }

    /// 把就绪的层自下而上叠成一张图，交给界面。
    fn composite_and_send(&mut self) {
        // 界面还没画完上一帧：这一帧丢掉，与主引擎的背压同一套语义。
        if self.pending.load(Ordering::SeqCst) {
            return;
        }
        let Some((width, height, canvas)) = self.canvas() else {
            return;
        };
        let buffer: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_raw(width, height, canvas).expect("画布尺寸由我们自己分配");
        let image = RenderImage::new([Frame::new(buffer)]);

        self.pending.store(true, Ordering::SeqCst);
        let _ = self.events.unbounded_send(PlayerEvent::Composite {
            image: Arc::new(image),
        });
    }

    /// 给截图 / 示波器回一份当前合成结果。
    fn snapshot(&self) -> Option<Snapshot> {
        let (width, height, pixels) = self.canvas()?;
        Some(Snapshot {
            width,
            height,
            pixels,
        })
    }
}
