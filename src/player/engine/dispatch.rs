//! 命令分发：把界面发来的 `Command` 翻译成 mpv 调用。

use super::*;

/// `Load` 的起点，以及「文件还没起来」这段窗口里收到的 seek。
///
/// `loadfile` 是异步的，命令返回成功时文件往往还没初始化完，这期间对 mpv 发
/// seek 会回 "error running command"：
///
/// - 紧跟着 `loadfile` 发 seek 会报错，界面上多出一条假的「播放出错」；更糟的
///   是 `?` 会提前返回，同一批命令里的「解除暂停」也一起漏发。
/// - 工程模式拖播放头时，同一个素材的后续拖动都走 `SeekAbsolute`。第一下
///   `Load` 之后 `timeline_preview` 已经记成这个文件，接下来几十下 seek 全部
///   落在加载窗口里，日志里就是连成串的「命令执行失败」，画面还停在旧位置。
///
/// 所以加载窗口内的 seek 不发命令，只把目标记下来（后到的覆盖先到的），等
/// `FILE_LOADED` 一次性落到用户最终停下的位置。
#[derive(Default)]
pub(super) struct PendingStart {
    /// `loadfile` 已排队、但还没收到 `FILE_LOADED`。
    loading: bool,
    /// 文件加载完成后要落到的位置；`None` 表示打开后从头开始。
    target: Option<f64>,
}

impl PendingStart {
    /// 换片 / 停止：上一次没能落地的起点作废，别带给下一个文件。
    pub(super) fn reset(&mut self) {
        self.loading = false;
        self.target = None;
    }

    /// 收到 `END_FILE` 时的处理。
    ///
    /// `loadfile replace` 会先给被替换掉的旧文件发一条 `reason = STOP` 的
    /// `END_FILE`，新的 `FILE_LOADED` 还在后面。这条事件不属于新文件，把刚登记
    /// 的起点清掉会同时坏两件事：起点的延时 seek 永远不会发生（画面从 0 开始），
    /// 加载窗口内的 `SeekAbsolute` 也不再被接管，直接落到尚未打开文件的 mpv 上，
    /// 日志里就是 `seek ... absolute exact` 失败。
    ///
    /// 真正的加载失败走 `reason = ERROR`，那条必须清（失败不会有 `FILE_LOADED`）。
    pub(super) fn end_file(&mut self, reason: i32) {
        if reason == ffi::end_file::STOP {
            return;
        }
        self.reset();
    }

    /// `loadfile` 成功后才登记起点，避免载入失败留下脏值。
    pub(super) fn begin(&mut self, start: Option<f64>) {
        self.loading = true;
        self.target = start.map(|seconds| seconds.max(0.0));
    }

    /// 文件还在加载：把 seek 目标存下来，返回 `true` 表示这条命令已被接管。
    ///
    /// 只保留最后一个目标——拖动播放头时中间那些位置用户早就滑过去了，
    /// 加载完成后直接落到松手处才有意义。
    pub(super) fn defer(&mut self, seconds: f64) -> bool {
        if !self.loading {
            return false;
        }
        self.target = Some(seconds.max(0.0));
        true
    }

    pub(super) fn is_loading(&self) -> bool {
        self.loading
    }

    /// 结束加载窗口并取走待落地的目标。拆出来是为了让状态机本身可以脱离
    /// mpv 句柄测试。
    fn take_loaded(&mut self) -> Option<f64> {
        self.loading = false;
        self.target.take()
    }

    /// 文件加载完成后补上延时 seek，只执行一次。失败只记日志：文件已经打开，
    /// 起点差一点不该弹「播放出错」横幅。
    pub(super) fn seek_loaded(&mut self, api: &MpvApi, handle: *mut ffi::MpvHandle) {
        let Some(seconds) = self.take_loaded() else {
            return;
        };
        let target = seconds.max(0.0).to_string();
        if let Err(error) = api.run_command(handle, &["seek", &target, "absolute"]) {
            log_warn!("定位起始位置失败: {error:#}");
        }
    }
}

pub(super) fn dispatch(
    api: &Arc<MpvApi>,
    handle: *mut ffi::MpvHandle,
    shared: &Arc<Shared>,
    command: Command,
    quit: &mut bool,
    pending_start: &mut PendingStart,
) {
    // 哪些命令失败该让用户看见：只有「用户明确要求的动作没做成」才算。
    // seek/选轨/设置这类失败在边界状态下很常见（片头片尾、流不可跳转），
    // 它们只写日志——否则只是按了一下方向键，界面上就弹一条「播放出错」。
    let report_failure = matches!(command, Command::Load { .. } | Command::AddSubtitle(_));
    let result = (|| -> Result<()> {
        match command {
            Command::Load { url, start } => {
                pending_start.reset();
                {
                    let mut state = shared.state();
                    state.error = None;
                    state.position = 0.0;
                    state.duration = 0.0;
                    state.video_width = 0;
                    state.video_height = 0;
                    state.tracks.clear();
                    state.chapters.clear();
                }
                shared.reset_frame();
                // 命令都没排上就别登记加载窗口：新文件不会起来，标志挂着会让
                // 之后所有 seek 被当成「还在加载」吞掉，画面彻底不动。
                if let Err(error) = api.run_command(handle, &["loadfile", &url, "replace"]) {
                    pending_start.reset();
                    return Err(error);
                }
                // 起点等 `FILE_LOADED` 再 seek，理由见 [`PendingStart`]。
                pending_start.begin(start);
                // 必须用命令、不能用 `set_property`：
                // `--pause` 是「打开文件后暂停」的选项，mpv 每次 loadfile 都会把它
                // 重新应用一遍；而 `set_property` 是即时的，会抢在排队的 loadfile
                // 之前执行完，于是文件加载时又被暂停回去——表现就是画面永远黑着。
                // 排进同一条命令队列才能保证「先 loadfile，再解除暂停」。
                api.run_command(handle, &["set", "pause", "no"])?;
            }
            Command::Stop => {
                pending_start.reset();
                {
                    let mut state = shared.state();
                    // path 变成不可用时属性通知会被忽略（见 apply_property），
                    // 这里主动清掉，否则界面会一直以为还有文件在播。
                    state.path = None;
                    state.video_width = 0;
                    state.video_height = 0;
                }
                shared.reset_frame();
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
                // 文件还在加载：现在发 seek 必定失败，记下目标等 `FILE_LOADED`。
                if pending_start.defer(target) {
                    return Ok(());
                }
                // 引擎空闲（还没打开文件，或刚 Stop）：没有可跳的片子。这不是
                // 用户的错，也不该每拖一下就在日志里刷一条「命令执行失败」。
                // `idle` 单独判断是因为加载失败后 `path` 的快照可能还慢半拍。
                let idle = {
                    let state = shared.state();
                    state.path.is_none() || state.idle
                };
                if idle {
                    return Ok(());
                }
                let ended = api.property_flag(handle, "eof-reached").unwrap_or(false);
                api.run_command(handle, &["seek", &target.to_string(), "absolute", "exact"])?;
                // 结尾处（keep-open 会把播放停住）往回拖通常是想接着看，
                // 这里顺手解除停住；平时暂停着拖进度条则保持暂停。
                if ended {
                    api.set_flag(handle, "pause", false)?;
                }
            }
            Command::SeekRelative(delta) => {
                // 相对位置基于「当前位置」，加载窗口里没有当前位置可加；
                // 而且加载完成后会落到用户最后指定的绝对位置，这里直接跳过。
                if pending_start.is_loading() || shared.state().path.is_none() {
                    return Ok(());
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 起点只在文件加载后消费一次且换片时作废() {
        let mut pending = PendingStart::default();
        assert!(pending.target.is_none());

        pending.begin(Some(12.5));
        assert!(pending.is_loading());
        assert_eq!(pending.take_loaded(), Some(12.5));
        assert!(!pending.is_loading());
        assert!(pending.target.is_none());

        // 上一次 loadfile 可能压根没成功，起点不能留给下一个文件。
        pending.begin(Some(3.0));
        pending.reset();
        assert!(!pending.is_loading());
        assert!(pending.target.is_none());
    }

    #[test]
    fn 加载窗口里的连续拖动只保留最后一个目标() {
        let mut pending = PendingStart::default();
        pending.begin(Some(1.0));

        // 加载还没完成：这些 seek 不该落到 mpv，只更新待落地目标。
        assert!(pending.defer(5.7));
        assert!(pending.defer(0.3));
        assert_eq!(pending.target, Some(0.3));

        // 加载完成后一次性落到最后一个位置，之后 seek 恢复即时执行。
        assert_eq!(pending.take_loaded(), Some(0.3));
        assert_eq!(pending.target, None);
        assert!(!pending.is_loading());
        assert!(!pending.defer(9.0));
    }

    #[test]
    fn 加载失败后不再吞掉后续的seek() {
        let mut pending = PendingStart::default();
        pending.begin(None);
        // 加载失败只会来一条 reason = ERROR 的 `END_FILE`，不会来 `FILE_LOADED`。
        pending.end_file(ffi::end_file::ERROR);
        assert!(!pending.defer(2.0));
    }

    #[test]
    fn 换片时旧文件的end_file不能清掉新起点() {
        let mut pending = PendingStart::default();
        pending.begin(Some(8.0));

        // `loadfile replace` 先给旧文件发 STOP，随后才是新文件的 `FILE_LOADED`。
        pending.end_file(ffi::end_file::STOP);
        assert!(pending.is_loading(), "换片窗口还没结束，起点必须留着");
        assert!(pending.defer(9.5), "加载窗口内的 seek 仍要被接管");
        assert_eq!(pending.take_loaded(), Some(9.5));
    }
}
