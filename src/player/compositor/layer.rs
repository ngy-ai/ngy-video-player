//! 合成里的一路：一个 mpv 实例 + 它那块按画布尺寸渲染的像素缓冲。

use crate::player::{
    ffi::{self, MpvApi, MpvRenderContext, MpvRenderParam, SendPtr, event, render_param},
    frame::{self, FrameBuffer},
};
use anyhow::{Result, anyhow};
use std::{
    ffi::{CString, c_void},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use super::{LOAD_TIMEOUT, SEEK_TIMEOUT, SEEK_TOLERANCE};

/// 一层画面要放什么。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LayerSpec {
    pub path: PathBuf,
    /// 素材内的时刻（秒）。
    pub source: f64,
    /// 这一层要不要跟着时间走。定格段与暂停时为假。
    pub playing: bool,
    /// 播放速率（1 = 原速）。J/K/L 变快时画面也得跟着快，否则合成出来的
    /// 那两路会按 1x 走，与按 2x / 4x 推进的播放头越离越远。
    ///
    /// 只给正数：倒放靠「按住 + 每拍补 seek」实现，由 `playing` 表达。
    pub rate: f64,
    /// 0 = 完全透明。
    pub opacity: f32,
}

/// 一层播放速率的合法范围：mpv 的 `speed` 认 0.01–100，这里收得比它更紧，
/// 与时间线 J/K/L 的档位（1 / 2 / 4）对得上；坏值退回原速。
pub(super) fn normalize_rate(rate: f64) -> f64 {
    if rate.is_finite() && rate > 0.0 {
        rate.clamp(0.1, 4.0)
    } else {
        1.0
    }
}

/// 一路 mpv：句柄、渲染上下文、像素缓冲。
pub(super) struct Layer {
    handle: SendPtr<ffi::MpvHandle>,
    context: *mut MpvRenderContext,
    pub(super) buffer: FrameBuffer,
    pub(super) media: Option<PathBuf>,
    pub(super) source: f64,
    /// 已经下发给 mpv 的速率：没变就不再发命令。
    rate: f64,
    pub(super) opacity: f32,
    /// 缓冲里是不是这一层的有效画面。换文件、改尺寸时置假。
    pub(super) ready: bool,
}

impl Layer {
    pub(super) fn new(api: &MpvApi) -> Result<Self> {
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
                rate: 1.0,
                opacity: 1.0,
                ready: false,
            })
        }
    }

    /// 换文件。等 `FILE_LOADED` 再往下走：`loadfile` 是异步的。
    pub(super) fn load(&mut self, api: &MpvApi, path: &Path) -> bool {
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
                Some(event::FILE_LOADED) => {
                    // `loadfile replace` 会先给旧文件发一条 `END_FILE(STOP)`，
                    // 上面那一步会顺手把 `media` 清掉；这里是新文件，补回来。
                    self.media = Some(path.to_path_buf());
                    return true;
                }
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
    pub(super) fn seek(&mut self, api: &MpvApi, time: f64, size: (u32, u32)) -> bool {
        // 跳转前先把「有新帧」这个标志消耗掉，否则第一次 update 看到的还是旧帧。
        self.discard_frame(api, size);

        let target = format!("{time:.3}");
        if let Err(error) = api.run_command(self.handle.get(), &["seek", &target, "absolute+exact"])
        {
            log_debug!("合成实例跳转失败: {error}");
            // 到过文件末尾、或者文件已被换掉时 seek 会一直失败：清掉媒体，
            // 下一次 present 会重新 loadfile，从「这一层永远不动」里恢复。
            self.forget_media();
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
    pub(super) fn close_to(&self, api: &MpvApi, time: f64) -> bool {
        api.property_f64(self.handle.get(), "time-pos")
            .is_some_and(|position| (position - time).abs() <= SEEK_TOLERANCE)
    }

    pub(super) fn has_frame(&self, api: &MpvApi) -> bool {
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
    pub(super) fn render(&mut self, api: &MpvApi, width: u32, height: u32) -> bool {
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

    pub(super) fn set_pause(&self, api: &MpvApi, paused: bool) {
        let value = if paused { "yes" } else { "no" };
        if let Err(error) = api.run_command(self.handle.get(), &["set", "pause", value]) {
            log_debug!("合成实例暂停失败: {error}");
        }
    }

    /// 播放速率。每帧都发一条 `set speed` 没必要，值没变就跳过。
    pub(super) fn set_speed(&mut self, api: &MpvApi, rate: f64) {
        let rate = normalize_rate(rate);
        if (self.rate - rate).abs() < 0.001 {
            return;
        }
        let value = format!("{rate:.3}");
        if let Err(error) = api.run_command(self.handle.get(), &["set", "speed", &value]) {
            log_debug!("合成实例变速失败: {error}");
            return;
        }
        self.rate = rate;
    }

    pub(super) fn unload(&mut self, api: &MpvApi) {
        if self.media.take().is_some() {
            let _ = api.run_command(self.handle.get(), &["stop"]);
        }
        self.ready = false;
    }

    /// 取一条事件。碰到 `END_FILE` 就把这一层降级成「没有素材」：mpv 到文件末尾
    /// 之后会回到 idle，**之后所有 seek 都会失败**（`error running command`），
    /// 而 `media` 还留着的话 `present` 认为「没换文件」不会重新加载，于是这一层
    /// 永远卡在最后一帧上。清空之后下一次 `present` 会重新 `loadfile`。
    pub(super) fn next_event(&mut self, api: &MpvApi, timeout: f64) -> Option<i32> {
        let event = unsafe { (api.wait_event)(self.handle.get(), timeout) };
        if event.is_null() {
            return None;
        }
        let id = unsafe { (*event).event_id };
        if id == event::END_FILE {
            self.forget_media();
        }
        Some(id)
    }

    /// 这一层手里没有可解的东西了：下次 `present` 按「换了文件」处理，重新加载。
    fn forget_media(&mut self) {
        if self.media.take().is_some() {
            log_debug!("合成实例已到文件末尾，下次展示时重新加载");
        }
        self.ready = false;
    }

    pub(super) fn destroy(&mut self, api: &MpvApi) {
        // 收尾顺序不能反：先放渲染上下文，再销毁实例。
        unsafe {
            (api.render_context_free)(self.context);
            (api.terminate_destroy)(self.handle.get());
        }
    }
}
