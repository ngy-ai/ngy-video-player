//! 命令分发：把界面发来的 `Command` 翻译成 mpv 调用。

use super::*;

pub(super) fn dispatch(
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
