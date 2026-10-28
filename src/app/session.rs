//! 会话入口：窗口状态、事件循环、空闲看门狗。

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
            playlists: PlaylistSet::load(),
            playing_tab: None,
            selection: BTreeSet::new(),
            selection_anchor: None,
            playlist_dragging: false,
            item_menu: None,
            file_info: None,
            settings,
            on_top_applied: false,
            focus,
            last_activity: Instant::now(),
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
            dialog_open: false,
            osd: None,
            screenshot_card: None,
            error: None,
            show_help: false,
            show_picture: false,
            show_engine: false,
            menu: None,
            menu_bar: None,
            mini: None,
            mini_window: None,
            main_restoring: false,
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
            if (speed - 1.0).abs() > f64::EPSILON {
                app.send(Command::SetSpeed(speed));
            }
            // 解码方式与显卡已经在 `Engine::spawn` 里作为选项下发过了。
            app.apply_picture();
        }

        app.consume_events(event_rx, cx);
        app.start_idle_watchdog(cx);
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

    /// 面板宽度：跟着窗口收，避免窗口缩小后面板把画面整块吃掉。
    pub(super) fn playlist_width(&self, window: &Window) -> f32 {
        let available = f32::from(window.viewport_size().width);
        let max = (available - VIDEO_MIN_WIDTH).clamp(PLAYLIST_MIN_WIDTH, PLAYLIST_MAX_WIDTH);
        self.settings.playlist_width.clamp(PLAYLIST_MIN_WIDTH, max)
    }

    /// 拖动面板左边界。面板贴右边，所以宽度就是「窗口宽 - 鼠标 x」。
    pub(super) fn drag_playlist_width(&mut self, x: Pixels, window: &Window) {
        let available = f32::from(window.viewport_size().width);
        let max = (available - VIDEO_MIN_WIDTH).clamp(PLAYLIST_MIN_WIDTH, PLAYLIST_MAX_WIDTH);
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
                    PlayerEvent::Frame(image) => {
                        if let Some(engine) = &this.engine {
                            // 告诉取帧线程「这一帧已被取走」，否则它会主动丢帧。
                            engine.frame_consumed();
                        }
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
                    PlayerEvent::Thumbnail { time, image } => {
                        this.on_thumbnail(time, image);
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
        self.controls_forced
            || self.last_activity.elapsed() < CONTROLS_IDLE
            // 暂停时一直留着：播放器停下的时候，用户的第一反应就是去找「播放」按钮，
            // 这时候把控制条藏起来等于让人以为界面上什么都没有。
            || self.snapshot.paused
            || self.menu.is_some()
            || self.menu_bar.is_some()
            || self.dragging.is_some()
            || self.volume_dragging
            || self.show_help
            || self.show_picture
            || self.show_engine
            || self.playlist().is_empty()
    }

    pub(super) fn poke(&mut self) {
        self.last_activity = Instant::now();
    }

    pub(super) fn toggle_help(&mut self, cx: &mut Context<Self>) {
        self.show_help = !self.show_help;
        cx.notify();
    }

    /// 播放列表面板的开合。关掉时顺手清掉选择：再打开时不该有一批
    /// 「关着的时候选的」行还亮着，Delete 也不该删到看不见的东西。
    pub(super) fn set_playlist_visible(&mut self, visible: bool) {
        self.settings.show_playlist = visible;
        if !visible {
            self.selection.clear();
            self.selection_anchor = None;
        }
    }
}
