//! 时间线预览的第二路声音：音频轨上的片段用**独立的 mpv 实例**放。
//!
//! 为什么另起一个实例：主实例里装着视频片段那个文件，mpv 一次只放一个文件的音频，
//! 也没法把「另一个文件的第 3 秒」混进来。再开一个纯音频实例，主实例继续管画面，
//! 两边各自解码，界面只负责在对的时机下命令。
//!
//! 线程模型与缩略图实例一致：句柄只在 `mpv-audio` 线程上使用，界面侧只投递请求。
//! 这个实例不要画面、不要字幕、不建窗口；音频设备不可用时 mpv 会退到空输出，
//! 也就是「没有声音但也不报错」——预览是锦上添花，不该因此打断播放。

use crate::player::ffi::{self, MpvApi, SendPtr, event};
use anyhow::{Context as _, Result, anyhow};
use std::{
    ffi::CString,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// 打开文件的等待上限：网络位置或刚拔掉的硬盘不该把线程卡死。
const LOAD_TIMEOUT: Duration = Duration::from_secs(4);
/// 同一文件里位置差多少才值得补一次 seek。
///
/// 比主画面那边宽一点：声音的 seek 会重新起一次音频输出，几十毫秒的偏差听不出来，
/// 但每几百毫秒补一次 seek 会一直「咔」。
const SEEK_TOLERANCE: Duration = Duration::from_millis(300);
/// 空闲时醒来的间隔：只为看一眼退出标志、顺手把事件队列排空。
const IDLE: Duration = Duration::from_millis(80);

/// 界面 → 音频实例的请求。
enum Request {
    /// 换文件（或同一文件内跳转），按 `playing` 决定放还是停在这一帧。
    Play {
        path: PathBuf,
        source: f64,
        playing: bool,
    },
    /// 暂停，文件留着（播放头可能很快又走回来）。
    Pause,
    /// 停下来并卸载文件。
    Stop,
    SetVolume(f64),
    /// 预览播放倍速：与主画面共用同一个播放速度，否则两边会越走越远。
    SetSpeed(f64),
    Shutdown,
}

/// 界面侧持有的句柄。丢掉它等于关掉整个音频实例。
pub(crate) struct AudioPreview {
    requests: mpsc::Sender<Request>,
    thread: Option<JoinHandle<()>>,
}

impl AudioPreview {
    pub(crate) fn spawn(api: Arc<MpvApi>) -> Result<Self> {
        let (requests, receiver) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("mpv-audio".into())
            .spawn(move || match Worker::new(api) {
                Ok(mut worker) => worker.run(&receiver),
                // 起不来就退化成「没有音频轨预览」，主画面不受影响。
                Err(error) => log_warn!("音频轨预览不可用: {error:#}"),
            })
            .context("启动音频预览线程")?;
        Ok(Self {
            requests,
            thread: Some(thread),
        })
    }

    pub(crate) fn play(&self, path: &Path, source: f64, playing: bool) {
        let _ = self.requests.send(Request::Play {
            path: path.to_path_buf(),
            source,
            playing,
        });
    }

    pub(crate) fn pause(&self) {
        let _ = self.requests.send(Request::Pause);
    }

    pub(crate) fn stop(&self) {
        let _ = self.requests.send(Request::Stop);
    }

    pub(crate) fn set_volume(&self, volume: f64) {
        let _ = self.requests.send(Request::SetVolume(volume));
    }

    pub(crate) fn set_speed(&self, speed: f64) {
        let _ = self.requests.send(Request::SetSpeed(speed));
    }
}

impl Drop for AudioPreview {
    fn drop(&mut self) {
        let _ = self.requests.send(Request::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Worker {
    api: Arc<MpvApi>,
    handle: SendPtr<ffi::MpvHandle>,
    /// 已经装着的文件；`None` 表示实例里什么都没有。
    media: Option<PathBuf>,
}

impl Worker {
    fn new(api: Arc<MpvApi>) -> Result<Self> {
        unsafe {
            let handle = (api.create)();
            if handle.is_null() {
                return Err(anyhow!("mpv_create 返回空指针"));
            }
            let handle = SendPtr(handle);

            // 这个实例只负责出声。画面、字幕、窗口全部关掉，少解一路视频，
            // 切片段时也就少一次「画面跟着闪」的机会。
            for (name, value) in [
                ("vo", "null"),
                ("vid", "no"),
                ("aid", "auto"),
                ("sid", "no"),
                ("sub-auto", "no"),
                ("audio-display", "no"),
                ("force-window", "no"),
                // 先别放：界面会在播放头走到音频片段时再放行。
                ("pause", "yes"),
                ("idle", "yes"),
                // 放完别卸文件：同一段音频常被反复试听。
                ("keep-open", "yes"),
                ("terminal", "no"),
                ("input-default-bindings", "no"),
                ("input-vo-keyboard", "no"),
                ("osd-level", "0"),
                ("hr-seek", "yes"),
                ("cache", "auto"),
                ("demuxer-max-bytes", "32MiB"),
                // 没有可用声卡时（远程桌面、容器）退到空输出，别让命令报错。
                ("audio-fallback-to-null", "yes"),
            ] {
                let Ok(cname) = CString::new(name) else {
                    continue;
                };
                let Ok(cvalue) = CString::new(value) else {
                    continue;
                };
                // 选项被拒绝不算失败：不同 mpv 版本认识的选项略有差异。
                (api.set_option_string)(handle.get(), cname.as_ptr(), cvalue.as_ptr());
            }

            let rc = (api.initialize)(handle.get());
            if rc < 0 {
                let message = api.error_text(rc);
                (api.terminate_destroy)(handle.get());
                return Err(anyhow!("音频预览实例初始化失败: {message}"));
            }

            Ok(Self {
                api,
                handle,
                media: None,
            })
        }
    }

    fn run(&mut self, requests: &mpsc::Receiver<Request>) {
        loop {
            match requests.recv_timeout(IDLE) {
                Ok(Request::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Ok(request) => self.handle(request),
                // 空闲时也要把事件队列排空，否则 mpv 会在内部堆到上限。
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            self.drain();
        }
        unsafe { (self.api.terminate_destroy)(self.handle.get()) };
    }

    fn handle(&mut self, request: Request) {
        match request {
            Request::Play {
                path,
                source,
                playing,
            } => self.play(&path, source, playing),
            Request::Pause => self.set_pause(true),
            Request::Stop => self.unload(),
            Request::SetVolume(volume) => self.set_volume(volume),
            Request::SetSpeed(speed) => self.set_speed(speed),
            Request::Shutdown => {}
        }
    }

    fn play(&mut self, path: &Path, source: f64, playing: bool) {
        if self.media.as_deref() != Some(path) {
            if !self.load(path) {
                return;
            }
            // 刚加载完的位置是 0：无条件跳一次，不能走「偏差不大就跳过」。
            self.seek_at(source, true);
        } else {
            self.seek_at(source, false);
        }
        self.set_pause(!playing);
    }

    fn load(&mut self, path: &Path) -> bool {
        let url = path.to_string_lossy().to_string();
        if let Err(error) = self
            .api
            .run_command(self.handle.get(), &["loadfile", &url, "replace"])
        {
            log_debug!("音频预览打开失败: {error}");
            self.media = None;
            return false;
        }
        self.media = Some(path.to_path_buf());

        // `loadfile` 是异步的：紧跟其后的 seek 会撞上「文件还没打开」。
        let deadline = Instant::now() + LOAD_TIMEOUT;
        while Instant::now() < deadline {
            match self.next_event(0.02) {
                Some(event::FILE_LOADED) => return true,
                // 打不开（格式不支持、文件没了）：安静放弃，别把错误甩给用户。
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

    /// 跳到 `source`。`force` 为真时不管当前位置都跳。
    fn seek_at(&mut self, source: f64, force: bool) {
        if self.media.is_none() {
            return;
        }
        if !force
            && let Some(current) = self.api.property_f64(self.handle.get(), "time-pos")
            && (current - source).abs() <= SEEK_TOLERANCE.as_secs_f64()
        {
            return;
        }
        let target = format!("{source:.3}");
        if let Err(error) = self
            .api
            .run_command(self.handle.get(), &["seek", &target, "absolute+exact"])
        {
            log_debug!("音频预览跳转失败: {error}");
        }
    }

    fn set_pause(&mut self, paused: bool) {
        let value = if paused { "yes" } else { "no" };
        if let Err(error) = self
            .api
            .run_command(self.handle.get(), &["set", "pause", value])
        {
            log_debug!("音频预览暂停失败: {error}");
        }
    }

    fn set_volume(&mut self, volume: f64) {
        // mpv 的音量上限在 130 左右，超出会报错；这里按主播放器的量程夹一下。
        let volume = volume.clamp(0.0, 130.0);
        let value = format!("{volume:.1}");
        if let Err(error) = self
            .api
            .run_command(self.handle.get(), &["set", "volume", &value])
        {
            log_debug!("音频预览设置音量失败: {error}");
        }
    }

    fn set_speed(&mut self, speed: f64) {
        // mpv 的 `speed` 对音频会做保持音高的变速（scaletempo），与主画面一致。
        let value = format!("{:.2}", speed.clamp(0.1, 8.0));
        if let Err(error) = self
            .api
            .run_command(self.handle.get(), &["set", "speed", &value])
        {
            log_debug!("音频预览设置倍速失败: {error}");
        }
    }

    fn unload(&mut self) {
        self.set_pause(true);
        if self.media.take().is_some() {
            let _ = self.api.run_command(self.handle.get(), &["stop"]);
        }
    }

    /// 排空事件队列；顺便把排队中的请求读掉。
    fn drain(&mut self) {
        while self.next_event(0.0).is_some() {}
    }

    fn next_event(&self, timeout: f64) -> Option<i32> {
        let event = unsafe { (self.api.wait_event)(self.handle.get(), timeout) };
        if event.is_null() {
            return None;
        }
        Some(unsafe { (*event).event_id })
    }
}
