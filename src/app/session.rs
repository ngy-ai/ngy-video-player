//! 会话入口：窗口状态、事件循环、空闲看门狗。

use super::video_area::frame_accepted;
use super::*;

impl PlayerApp {
    pub fn new(options: LaunchOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = Settings::load();
        let focus = cx.focus_handle();

        let (event_tx, event_rx) = futures::channel::mpsc::unbounded();
        // 解码方式与显卡走 `initialize` 之前的选项：此刻还没有 VO，
        // 用运行期的 `set` 命令不一定生效。
        let engine_options = EngineOptions {
            hwdec: settings.decode.hwdec().to_string(),
            d3d11_adapter: settings.gpu.clone(),
        };
        let (engine, startup_failure) = match Engine::spawn(event_tx, &engine_options) {
            Ok(engine) => (Some(Arc::new(engine)), None),
            Err(error) => {
                log_error!("初始化播放引擎失败: {error:#}");
                (None, Some(format!("{error:#}")))
            }
        };

        let mut app = Self {
            engine,
            snapshot: Snapshot {
                volume: settings.volume,
                muted: settings.muted,
                speed: settings.speed,
                ..Snapshot::default()
            },
            frame: None,
            incoming: None,
            retired: VecDeque::new(),
            frame_epoch: 0,
            loading_audio_only: false,
            playlists: PlaylistSet::load(),
            playing_tab: None,
            selection: BTreeSet::new(),
            selection_anchor: None,
            playlist_dragging: false,
            panel_menu: None,
            drop_spot: None,
            item_menu: None,
            tab_menu: None,
            tab_rename: None,
            file_info: None,
            clip: None,
            clip_generation: 0,
            convert: None,
            convert_generation: 0,
            ab_loop: AbLoop::default(),
            subtitle_delay: 0.0,
            remux: None,
            remux_generation: 0,
            metadata: None,
            metadata_generation: 0,
            proxy: None,
            proxy_generation: 0,
            proxy_queue: None,
            proxy_queue_generation: 0,
            cache_queue: None,
            cache_queue_generation: 0,
            cache_entries: Vec::new(),
            cache_bytes: 0,
            scope: None,
            scope_generation: 0,
            scenes: None,
            scenes_generation: 0,
            multicam: None,
            multicam_generation: 0,
            concat: None,
            concat_generation: 0,
            settings,
            on_top_applied: false,
            focus,
            last_activity: Instant::now(),
            started: Instant::now(),
            controls_forced: true,
            dragging: None,
            progress_bounds: Rc::new(Cell::new(Bounds::default())),
            volume_bounds: Rc::new(Cell::new(Bounds::default())),
            volume_dragging: false,
            speed_bounds: Rc::new(Cell::new(Bounds::default())),
            speed_dragging: false,
            frame_scroll: 0.0,
            hover: None,
            preview_request: None,
            preview_retired: VecDeque::new(),
            project: None,
            history: None,
            show_history: false,
            project_path: None,
            project_saved: None,
            autosave: AutoSave::new(),
            playlist_import: None,
            library: None,
            library_import: None,
            library_generation: 0,
            audition: None,
            timeline: ViewState::new(),
            timeline_dragging: false,
            timeline_snap: false,
            timeline_drop: None,
            asset_drop: None,
            keyframe_drop: None,
            trim_drop: None,
            trim_tool: TrimTool::default(),
            trim_window: None,
            trim_token: 0,
            timeline_menu: None,
            curve_drop: None,
            curve_select: None,
            speed_drop: None,
            key_clipboard: None,
            timeline_preview: None,
            timeline_playing: false,
            timeline_last_tick: Instant::now(),
            timeline_loaded_at: None,
            timeline_clock: 0,
            timeline_audio: Vec::new(),
            timeline_audio_failed: false,
            timeline_audio_active: false,
            audio_depth_warned: false,
            compositor: None,
            composite_layers: Vec::new(),
            composite_failed: false,
            composite_depth_warned: false,
            timeline_shuttle: 0.0,
            shuttle_key_held: false,
            scrub_audio: false,
            scrub_tail: 0,
            timecode_dialog: None,
            export_dialog: None,
            subtitle_dialog: None,
            export_settings: ExportSettings::default(),
            export_task: None,
            export_progress: None,
            export_preset: None,
            export_sequence: None,
            export_queue: VecDeque::new(),
            dialog_open: false,
            project_confirm: None,
            delete_track_confirm: None,
            sequence_dialog: None,
            sequence_manager: None,
            source_asset: None,
            source_in: None,
            source_out: None,
            pending_sequence_dialog: None,
            show_stats: false,
            osd: None,
            screenshot_card: None,
            error: None,
            show_settings: false,
            settings_tab: SettingsTab::default(),
            menu: None,
            menu_bar: None,
            mini: None,
            mini_window: None,
            main_restoring: false,
            mini_restore_fullscreen: false,
            main_window: window.window_handle(),
            association_status: platform::associations::Status::default(),
            gpu_adapters: platform::gpu::adapters(),
            startup_failure,
        };

        // 应用保存的音量/静音/速度。
        if app.engine.is_some() {
            let volume = app.settings.volume;
            let muted = app.settings.muted;
            let speed = app.settings.speed;
            app.send(Command::SetVolume(volume));
            app.send(Command::SetMuted(muted));
            // 无条件下发：mpv 的初始值可能来自用户自己的 mpv.conf，不能假设是 1.0。
            // 速度是全局偏好，所有视频共用这一个值。
            app.send(Command::SetSpeed(speed));
            // 解码方式与显卡已经在 `Engine::spawn` 里作为选项下发过了。
            app.apply_picture();
            // HDR 素材在预览里也走 hable（ADR 0007）：和导出的 tonemap 曲线对上，
            // 预览里调好的曝光导出后不会变样。
            app.send(Command::SetOption {
                name: "tone-mapping".to_string(),
                value: "hable".to_string(),
            });
        }

        app.consume_events(event_rx, cx);
        app.start_idle_watchdog(cx);
        app.start_autosave_watch(cx);
        app.auto_maintain_associations();

        // 小窗模式的不变式：同一时刻只有主窗口或小窗之一可见。
        //
        // 主窗口变可见并不表示「退出小窗」：用户想看的是播放器，而此刻播放器就是
        // 那个小窗。所以把它重新收起来，再把小窗抬到前台，让这个手势落在小窗上。
        // 真正退出小窗只有小窗上的还原按钮 / Esc / T，那几条路径会先置上
        // `main_restoring`。
        //
        // 正常情况下这条路径根本不会触发：主窗口进入小窗模式时就被藏起来了，任务栏
        // 上属于小窗（见 `open_mini` 与 `platform::window::can_hide`），没有哪个按钮
        // 能把主窗口叫出来。藏不起来的那两个平台上它仍是「点任务栏」的兜底：那里
        // 只能事后纠正，主窗口会闪一下。
        // 注册时会立刻回调一次，那时还没有小窗，是空操作。
        cx.observe_window_visibility(window, |app, visibility, window, cx| {
            if !matches!(visibility, WindowVisibility::Visible)
                || app.mini.is_none()
                || app.main_restoring
            {
                return;
            }
            if !platform::window::hide(window) {
                window.minimize_window();
            }
            app.focus_mini(cx);
        })
        .detach();

        if options.paths.is_empty() {
            app.restore_session();
        } else {
            app.open_paths(options.paths, cx);
        }
        if options.fullscreen {
            window.toggle_fullscreen();
        }

        app.focus.focus(window, cx);
        app
    }

    // -- 引擎交互 ----------------------------------------------------------

    pub(super) fn send(&self, command: Command) {
        if let Some(engine) = &self.engine {
            engine.send(command);
        }
    }

    // -- 列表访问 ----------------------------------------------------------

    /// 当前显示页的列表。读操作一律走这里。
    pub(super) fn playlist(&self) -> &Playlist {
        self.playlists.active_playlist()
    }

    pub(super) fn playlist_mut(&mut self) -> &mut Playlist {
        self.playlists.active_playlist_mut()
    }

    /// 正在播的那一页。`None` 表示引擎里可能还有东西在放，但已经没有播放队列了
    /// （正在播的列表被关掉、清空或删光）。这时换集和自动续播都停下来，
    /// 等用户重新从列表里点一个文件——列表的增删合闭永远不打断播放。
    pub(super) fn playback_tab(&self) -> Option<usize> {
        self.playing_tab
    }

    /// 右侧面板占的宽度：播放器模式是裁切 / 转换 / 播放列表互斥，工程模式只有
    /// 编辑历史侧栏。同一时刻最多一条，宽度只能在这里算一次。
    ///
    /// 画面、控制条、菜单都按这个数让位——三处各算一次迟早会有一处忘了改，
    /// 表现为画面被面板盖住一条。
    pub(super) fn side_panel_width(&self, window: &Window) -> f32 {
        // 全屏只放画面：右侧这条通道整条让出来，画面与控制条铺满整屏。
        // 面板自己的状态不动（裁切/转换跑到一半的任务、列表的开关都还在），
        // 退出全屏原样回来。
        if window.is_fullscreen() {
            return 0.0;
        }
        // 工程模式只有「预览 + 时间线」两块，右侧通道平时整条让出来；
        // 编辑历史侧栏展开时才占一条，画面按这个数让位。
        if self.project.is_some() {
            if !self.show_history {
                return 0.0;
            }
            // 窄窗口先保住预览：至少给画面留 240 像素。
            let available = f32::from(window.viewport_size().width);
            return HISTORY_PANEL_WIDTH.min((available - 240.0).max(120.0));
        }
        // 裁切与转换都是定宽面板，宽度规则一模一样。
        let fixed = if self.convert.is_some() {
            Some(CONVERT_PANEL_WIDTH)
        } else if self.clip.is_some() {
            Some(CLIP_PANEL_WIDTH)
        } else {
            None
        };
        if let Some(width) = fixed {
            // 与控制条共享宽度：面板再宽也不能把控制条右侧按钮挤出窗口。
            let available = f32::from(window.viewport_size().width);
            width.min((available - CONTROLS_MIN_WIDTH).max(240.0))
        } else if self.settings.show_playlist {
            self.playlist_width(window)
        } else {
            0.0
        }
    }

    /// 面板宽度：跟着窗口收，避免窗口缩小后面板把画面整块吃掉。
    pub(super) fn playlist_width(&self, window: &Window) -> f32 {
        let available = f32::from(window.viewport_size().width);
        self.settings
            .playlist_width
            .clamp(PLAYLIST_MIN_WIDTH, Self::playlist_max_width(available))
    }

    /// 面板最多能占多宽。
    ///
    /// 控制条让开面板之后剩下的那一条是定宽的，压到 `CONTROLS_MIN_WIDTH`
    /// 以下时右侧按钮就会越过面板边界、压在列表行上。窗口特别窄时也只能
    /// 保住 `PLAYLIST_MIN_WIDTH`，剩下的交给控制条自己去裁。
    fn playlist_max_width(available: f32) -> f32 {
        (available - CONTROLS_MIN_WIDTH).clamp(PLAYLIST_MIN_WIDTH, PLAYLIST_MAX_WIDTH)
    }

    /// 拖动面板左边界。面板贴右边，所以宽度就是「窗口宽 - 鼠标 x」。
    pub(super) fn drag_playlist_width(&mut self, x: Pixels, window: &Window) {
        let available = f32::from(window.viewport_size().width);
        let max = Self::playlist_max_width(available);
        self.settings.playlist_width = (available - f32::from(x)).clamp(PLAYLIST_MIN_WIDTH, max);
    }

    pub(super) fn consume_events(
        &self,
        mut receiver: UnboundedReceiver<PlayerEvent>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            while let Some(event) = receiver.next().await {
                let alive = this.update(cx, |this, cx| match event {
                    PlayerEvent::Dirty => {
                        if let Some(engine) = &this.engine {
                            this.snapshot = engine.snapshot();
                        }
                        cx.notify();
                    }
                    PlayerEvent::Frame { image, epoch } => {
                        if let Some(engine) = &this.engine {
                            // 告诉取帧线程「这一帧已被取走」，否则它会主动丢帧。
                            engine.frame_consumed();
                        }
                        // 合成器接管时单路预览已经让位：它迟到的帧贴上去就把多轨
                        // 画面盖掉了，而且暂停时不会再有新帧来纠正。
                        let compositing = !this.composite_layers.is_empty();
                        // 换片之前渲染、却晚于 FrameCleared 才送达的旧帧一律丢掉：
                        // 放回去就是「播着音频，画面还是上一个视频」。
                        // 纯音频再单独挡一道：mpv 在 loadfile 期间会把它手里
                        // 上一部片子的最后一帧按新代次重新渲染出来，连代次都对得上。
                        if !compositing
                            && frame_accepted(epoch, this.frame_epoch, this.loading_audio_only)
                        {
                            this.incoming = Some(image.clone());
                            // 小窗不自己解码，只是同一张帧的第二个窗口，这里转发一份。
                            if let Some(mini) = &this.mini {
                                mini.update(cx, |mini, cx| {
                                    mini.incoming = Some(image);
                                    cx.notify();
                                });
                            }
                            cx.notify();
                        }
                    }
                    PlayerEvent::FrameCleared(epoch) => {
                        this.frame_epoch = epoch;
                        this.discard_frame();
                        if let Some(mini) = &this.mini {
                            mini.update(cx, |mini, cx| {
                                mini.discard_frame();
                                cx.notify();
                            });
                        }
                        // 引擎已经把分辨率归零，界面这份跟着对齐，
                        // 纯音频立刻按「无画面」处理，不等下一次 Dirty。
                        if let Some(engine) = &this.engine {
                            this.snapshot = engine.snapshot();
                        }
                        cx.notify();
                    }
                    PlayerEvent::Thumbnail { time, image } => {
                        this.on_thumbnail(time, image);
                        cx.notify();
                    }
                    PlayerEvent::Composite { image } => {
                        // 告诉合成线程「这一帧已被取走」，否则它会一直等着丢帧。
                        if let Some(compositor) = &this.compositor {
                            compositor.frame_consumed();
                        }
                        this.incoming = Some(image.clone());
                        // 小窗也是同一张画面：转发一份，不自己再解一次。
                        if let Some(mini) = &this.mini {
                            mini.update(cx, |mini, cx| {
                                mini.incoming = Some(image);
                                cx.notify();
                            });
                        }
                        cx.notify();
                    }
                    PlayerEvent::Ended(reason) => {
                        this.on_ended(reason, cx);
                    }
                    PlayerEvent::Error(message) => {
                        log_warn!("播放器错误: {message}");
                        this.error = Some(Banner::new("播放出错", message));
                        cx.notify();
                    }
                    PlayerEvent::Screenshot(result) => {
                        match result {
                            Ok(path) => {
                                log_info!("截图已保存: {}", path.display());
                                // 用卡片代替 OSD：这里还要让用户能复制、打开、定位。
                                this.show_screenshot_card(path, cx);
                            }
                            Err(message) => {
                                log_warn!("截图失败: {message}");
                                this.flash(format!("截图失败：{}", error_reason(&message)));
                            }
                        }
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// 控制条自动隐藏：不用「每帧检查」，而是低频轮询，省得每次鼠标移动都
    /// 重新起一个定时器。
    pub(super) fn start_idle_watchdog(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
                let alive = this.update(cx, |this, cx| {
                    let mut dirty = false;
                    if this
                        .osd
                        .as_ref()
                        .is_some_and(|(_, at)| at.elapsed() > OSD_LIFETIME)
                    {
                        this.osd = None;
                        dirty = true;
                    }
                    if this.controls_visible() && this.last_activity.elapsed() > CONTROLS_IDLE {
                        this.controls_forced = false;
                        dirty = true;
                    }
                    // 鼠标停在卡片上时不算超时：用户正要点按钮。
                    if this.screenshot_card.as_ref().is_some_and(|card| {
                        !card.hovered && card.shown_at.elapsed() > SCREENSHOT_CARD_LIFETIME
                    }) {
                        this.clear_screenshot_card();
                        dirty = true;
                    }
                    if dirty {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    // -- 播放控制 ----------------------------------------------------------

    pub(super) fn controls_visible(&self) -> bool {
        // 工程模式是「预览 + 时间线」，播放控制条整条不画：进度与定位都交给时间线。
        if self.project.is_some() {
            return false;
        }
        self.controls_forced
            || self.last_activity.elapsed() < CONTROLS_IDLE
            // 暂停时一直留着：播放器停下的时候，用户的第一反应就是去找「播放」按钮，
            // 这时候把控制条藏起来等于让人以为界面上什么都没有。
            || self.snapshot.paused
            || self.menu.is_some()
            || self.menu_bar.is_some()
            || self.dragging.is_some()
            || self.volume_dragging
            || self.show_settings
            || self.playlist().is_empty()
    }

    pub(super) fn poke(&mut self) {
        self.last_activity = Instant::now();
    }

    /// 播放列表面板的开合（按钮、`L`、菜单共用这一个入口）。
    ///
    /// 全屏里右侧通道不画，`show_playlist` 却还是「开」的。这时候用户按列表
    /// 按钮，本意就是「把列表拿出来」，不是「关掉一个看不见的开关」——所以
    /// 先把窗口退回全屏之前的样子，再把列表打开。
    pub(super) fn toggle_playlist(&mut self, window: &mut Window) {
        if window.is_fullscreen() {
            self.toggle_fullscreen(window);
            self.set_playlist_visible(true);
        } else {
            self.set_playlist_visible(!self.settings.show_playlist);
        }
    }

    /// 播放列表面板的开合。关掉时顺手清掉选择：再打开时不该有一批
    /// 「关着的时候选的」行还亮着，Delete 也不该删到看不见的东西。
    pub(super) fn set_playlist_visible(&mut self, visible: bool) {
        self.settings.show_playlist = visible;
        if visible {
            // 裁切 / 转换面板与列表共用右侧通道：切到列表就把它们收起来。
            // 任务一并取消，半成品由任务线程删掉。
            self.dismiss_clip_panel();
            self.dismiss_convert_panel();
        }
        if !visible {
            self.selection.clear();
            self.selection_anchor = None;
            // 面板收起来时菜单也一起收掉，否则下次展开它还挂在那里。
            self.panel_menu = None;
            self.drop_spot = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 面板和控制条并排：面板最多只能占到「窗口宽 - 控制条所需宽度」，
    /// 否则控制条右侧的按钮会越界压在列表行上。
    #[test]
    fn playlist_never_squeezes_the_controls() {
        for window in [1920.0_f32, 1280.0, 1120.0, 1000.0] {
            let panel = PlayerApp::playlist_max_width(window);
            assert!(
                window - panel >= CONTROLS_MIN_WIDTH,
                "窗口 {window} 宽时面板占到 {panel}，控制条只剩 {}",
                window - panel
            );
        }
    }

    /// 窗口窄到连控制条都排不下时，面板退到最小宽度为止；
    /// 再窄只能由控制条自己裁，不能反过来挤没画面。
    #[test]
    fn narrow_window_keeps_the_playlist_at_its_minimum() {
        assert_eq!(PlayerApp::playlist_max_width(700.0), PLAYLIST_MIN_WIDTH);
    }
}
