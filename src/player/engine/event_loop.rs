//! 引擎线程主循环：等 mpv 事件、更新共享快照、必要时通知界面。

use super::*;

// ---------------------------------------------------------------------------
// 事件线程
// ---------------------------------------------------------------------------

pub(super) fn event_loop(
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
