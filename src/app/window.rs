//! 窗口形态：全屏、迷你窗、置顶。

use super::*;

impl PlayerApp {
    pub(super) fn toggle_fullscreen(&mut self, window: &mut Window) {
        window.toggle_fullscreen();
        // 进出全屏时平台会重排窗口层级，之前设过的置顶可能被清掉；下一帧重设一次。
        self.on_top_applied = false;
    }

    // -- 小窗 --------------------------------------------------------------

    pub(super) fn toggle_mini(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_mini_visible(self.mini.is_none(), window, cx);
        cx.notify();
    }

    pub(super) fn set_mini_visible(
        &mut self,
        visible: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if visible {
            self.open_mini(window, cx);
        } else {
            self.dismiss_mini(cx);
        }
    }

    /// 进入小窗模式：另开一个置顶小窗口放画面，然后把主窗口最小化。
    ///
    /// 之所以不是「把主窗口缩小」：缩小之后播放列表、控制条、菜单栏全得挤进
    /// 几百像素里，等于换一种难用，而且窗口尺寸剧变会让 mpv 反复重设渲染目标。
    /// 弹一个独立小窗，主窗口的一切状态原样保留，还原是瞬时的。
    pub(super) fn open_mini(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mini.is_some() {
            return;
        }
        // 上一轮还原可能还没来得及走到 `mini_closed`；新会话必须从「没在还原」起步，
        // 否则接下来那次「点任务栏」会被误判成我们自己发起的还原而放过去。
        self.main_restoring = false;

        // 小窗模式与全屏互斥：不可见的主窗口停在全屏状态没有意义，
        // 还原回来还会落在一个说不清是全屏还是窗口的中间态。
        if window.is_fullscreen() {
            window.toggle_fullscreen();
        }

        let screen = self.screen_bounds(window, cx);
        let width = self.mini_width(screen);
        let height = width / self.mini_aspect();
        let origin = point(
            screen.origin.x + screen.size.width - px(width) - px(MINI_MARGIN),
            // 底部多留一段给任务栏 / Dock，否则小窗会压在它上面。
            screen.origin.y + screen.size.height - px(height) - px(MINI_BOTTOM_MARGIN),
        );

        // 窗口一建出来就会立刻画第一帧，那时 `self` 还被可变借用着，
        // 所以初始状态要在这里先算好带进去，而不是让小窗自己去读。
        let state = self.mini_state();
        // 置顶与否跟着上次的图钉走。窗口本身是 `PopUp`（为了不进任务栏），
        // 创建时带着 `WS_EX_TOPMOST`；不希望置顶时由小窗第一帧的 `sync_on_top`
        // 把它摘掉。
        let on_top = self.settings.mini_on_top;
        let app = cx.weak_entity();
        let mut opened: Option<Entity<MiniPlayer>> = None;
        // 建窗口的回调里要把视图句柄传出来，所以借用而不是 move 进去。
        let slot = &mut opened;
        let handle = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    origin,
                    size(px(width), px(height)),
                ))),
                // `titlebar: None` 在 Windows 后端等价于 `hide_title_bar = true`，
                // 也就是**不要系统标题栏**，正好是小窗要的样子；标题改由悬浮条自己画。
                // 注意 `Some(TitlebarOptions { appears_transparent: false })` 是反过来的，
                // 那个值会让系统把非客户区加回来。
                titlebar: None,
                // 工具窗口：任务栏和 Alt+Tab 里都不出现。小窗模式下任务栏只该剩主窗口
                // 那一条，多一条会让「点任务栏」这个手势含糊不清；那一条被点中时
                // 由可见性观察者把小窗重新抬回前台（见 `PlayerApp::new`）。
                kind: WindowKind::PopUp,
                // 主窗口马上要最小化，键盘必须落在小窗上。
                focus: true,
                is_resizable: true,
                window_min_size: Some(size(px(MINI_MIN_WIDTH), px(90.0))),
                ..Default::default()
            },
            |_window, cx| {
                let view = cx.new(|cx| MiniPlayer::new(app, state, on_top, cx));
                *slot = Some(view.clone());
                view
            },
        );

        match handle {
            Ok(handle) => {
                self.mini = opened;
                self.mini_window = Some(handle);
                // 主窗口被释放（用户关了主窗口）时，小窗也要跟着关：
                // 播放器状态只有主窗口那一份，留一个没人驱动的空窗口在屏幕上
                // 既不能操作，也没有别的办法关掉。
                let this = cx.entity();
                if let Some(view) = &self.mini {
                    let app = this.clone();
                    view.update(cx, |_mini, cx| {
                        cx.observe_release_in(&this, window, |mini, _app, _window, cx| {
                            mini.app_gone = true;
                            cx.notify();
                        })
                        .detach();
                        // 主窗口每次 notify 之后小窗自己拉一份状态。方向必须是这个：
                        // 小窗的按钮回调正跑在 `MiniPlayer` 的 update 里，主窗口若
                        // 反过来 push，就会把它 lease 第二次而 panic。
                        cx.observe(&app, |mini, app, cx| mini.pull_state(&app, cx))
                            .detach();
                    });
                }
                // 小窗不管是被系统关闭按钮关掉的、还是自己点 X 关的，都会走到这里；
                // 界面侧的引用统一在这收尾，否则再按一次 T 就是去关一个已经关掉的窗口。
                let id = handle.window_id();
                let this = cx.weak_entity();
                cx.on_window_closed(move |cx, closed| {
                    if closed != id {
                        return;
                    }
                    if let Some(this) = this.upgrade() {
                        this.update(cx, |app, _cx| app.mini_closed());
                    }
                })
                .detach();
                // 先把小窗显示出来，再收起主窗口：反过来的话有一瞬间屏幕上什么都没有。
                window.minimize_window();
                self.flash("小窗：开");
            }
            Err(error) => {
                log_error!("打开小窗失败: {error:#}");
                self.flash("打开小窗失败");
            }
        }
    }

    /// 让主窗口重新可见（并回到前台）。这是「还原」的核心一步。
    ///
    /// 走 `update_window` 而不是 `WindowHandle::update`：后者会把 `PlayerApp` 再 lease
    /// 一次，而这个方法本身就跑在 `PlayerApp` 的借用里，会直接 panic。
    /// gpui 的 `activate` 在 Windows 上顺带 `SW_RESTORE`，最小化的窗口会一并恢复。
    pub(super) fn reactivate_main(&mut self, cx: &mut Context<Self>) {
        // 这次「主窗口变可见」是我们自己要的，别让可见性观察者又把它收回去。
        self.main_restoring = true;
        let handle = self.main_window;
        let _ = cx.update_window(handle, |_root, window, _cx| {
            // gpui 的 `activate_window` 只做线程内的激活，不会把窗口抬到别的进程
            // 之上，从最小化还原后主窗口会停在别的应用后面。Windows 上走自己的实现。
            if !platform::window::bring_to_front(window) {
                window.activate_window();
            }
        });
    }

    /// 把前台交还给小窗。
    ///
    /// 用户点任务栏上那一条时，系统会先还原并激活主窗口；这里紧跟着把小窗抬回
    /// 最前，让这个手势最终落在小窗上，而不是把主窗口翻出来。
    pub(super) fn focus_mini(&mut self, cx: &mut Context<Self>) {
        let Some(handle) = self.mini_window else {
            return;
        };
        let handle: AnyWindowHandle = handle.into();
        let _ = cx.update_window(handle, |_root, window, _cx| {
            if !platform::window::bring_to_front(window) {
                window.activate_window();
            }
        });
    }

    /// 让小窗自己关掉。真正的销毁在它的下一次 render 里做——只有那儿拿得到
    /// `&mut Window`。引用与偏好由 `mini_closed` 收尾。
    pub(super) fn dismiss_mini(&mut self, cx: &mut Context<Self>) {
        let Some(mini) = &self.mini else {
            return;
        };
        mini.update(cx, |mini, cx| {
            if mini.dismissed {
                return;
            }
            mini.dismissed = true;
            cx.notify();
        });
    }

    /// 小窗关掉之后的收尾。窗口已经销毁，这里只清界面侧的引用。
    pub(super) fn mini_closed(&mut self) {
        self.mini = None;
        self.mini_window = None;
        // 小窗没了，主窗口的可见性重新回到「用户说了算」，下一次变可见照常处理。
        self.main_restoring = false;
        self.settings.save();
        self.flash("小窗：关");
    }

    /// 小窗要显示的那点状态，见 `MiniState`。
    pub(super) fn mini_state(&self) -> MiniState {
        MiniState {
            title: if self.snapshot.media_title.is_empty() {
                SharedString::from("小窗")
            } else {
                SharedString::from(self.snapshot.media_title.clone())
            },
            position: self.progress_value(),
            duration: self.snapshot.duration,
            playing: !self.snapshot.paused,
            volume: self.snapshot.volume,
            muted: self.snapshot.muted,
            empty: self.playlist().is_empty(),
        }
    }

    /// 小窗拖进度条：直接跳到某一秒。
    pub(super) fn seek_to(&mut self, position: f64, cx: &mut Context<Self>) {
        self.send(Command::SeekAbsolute(position));
        self.poke();
        cx.notify();
    }

    /// 小窗音量滑块。
    pub(super) fn set_volume_from_ui(&mut self, volume: f64, cx: &mut Context<Self>) {
        if self.snapshot.muted && volume > 0.0 {
            self.snapshot.muted = false;
            self.send(Command::SetMuted(false));
        }
        self.set_volume(volume);
        cx.notify();
    }

    /// 小窗宽度。用户拖过就按拖过的来，但最多占屏幕宽的 40%——
    /// 换到更小的屏幕上时，别让「小窗」变成半个桌面。
    pub(super) fn mini_width(&self, screen: Bounds<Pixels>) -> f32 {
        let limit = (f32::from(screen.size.width) * 0.4).max(MINI_MIN_WIDTH);
        self.settings
            .mini_width
            .clamp(MINI_MIN_WIDTH, MINI_MAX_WIDTH)
            .min(limit)
    }

    /// 小窗的高宽比：跟着画面设置走，旋转 90/270 度时长宽互换。
    pub(super) fn mini_aspect(&self) -> f32 {
        let ratio = self
            .settings
            .video_aspect
            .ratio()
            .unwrap_or_else(|| self.snapshot.aspect());
        let ratio = if self.settings.video_rotate.swaps_axes() {
            ratio.recip()
        } else {
            ratio
        };
        ratio.clamp(0.4, 4.0)
    }

    /// 新窗口该落在哪块屏幕上：跟着主窗口所在的那块，多屏时不会跑到别处。
    pub(super) fn screen_bounds(&self, window: &Window, cx: &App) -> Bounds<Pixels> {
        window
            .display(cx)
            .or_else(|| cx.primary_display())
            .map(|display| display.bounds())
            .unwrap_or_else(|| Bounds::new(point(px(0.0), px(0.0)), size(px(1280.0), px(720.0))))
    }

    /// 置顶由「偏好」和「当前播放状态」共同决定。
    pub(super) fn wants_on_top(&self) -> bool {
        match self.settings.on_top {
            OnTop::Never => false,
            // 暂停、空载、放完都不算「正在播」——这时还压着别的窗口只会碍事。
            OnTop::Playing => !self.snapshot.idle && !self.snapshot.paused,
            OnTop::Always => true,
        }
    }

    /// 期望值和上次下发的值不一致时才动窗口层级。
    ///
    /// 放到 `window.defer` 里执行：`SetWindowPos` 会同步派发窗口消息，在渲染途中
    /// 改 Z 序有重入风险，等这一帧收尾再做最稳。
    pub(super) fn sync_on_top(&mut self, window: &Window, cx: &mut Context<Self>) {
        let want = self.wants_on_top();
        if want == self.on_top_applied {
            return;
        }
        self.on_top_applied = want;
        window.defer(cx, move |window, _cx| {
            if !platform::topmost::set(window, want) {
                log_warn!("窗口置顶未生效：当前窗口系统不支持");
            }
        });
    }

    pub(super) fn set_on_top(&mut self, mode: OnTop) {
        if self.settings.on_top == mode {
            return;
        }
        self.settings.on_top = mode;
        // 这是「下次启动也照这个策略」的偏好，不能等窗口关闭才落盘。
        self.settings.save();
        self.flash(format!("置顶：{}", mode.label()));
    }

    // -- 画面设置 ----------------------------------------------------------
}
