//! 迷你播放器：独立小窗、悬停控制条与拖动定位。

use super::*;

/// 小窗：主窗口之外浮着的一个小播放器窗口。
///
/// 为什么不另起一个 mpv 实例：那样要多一份解码开销，音频会同时响两遍，
/// 两边的进度也会各自漂移。这里只显示主窗口转发过来的同一张帧，
/// 解码、音频、进度都还是主窗口那一份。
pub(super) struct MiniPlayer {
    /// 主窗口的视图。用弱引用：它关掉之后这里要能察觉到并跟着退出。
    pub(super) app: WeakEntity<PlayerApp>,
    /// 标题、进度等由主窗口推送的显示状态。
    pub(super) state: MiniState,
    pub(super) frame: Option<Arc<RenderImage>>,
    pub(super) incoming: Option<Arc<RenderImage>>,
    /// 已经画过、等待归还给 GPU atlas 的旧帧。规则与主窗口一致（留两帧余量），
    /// 但两窗口的 atlas 是各一份，所以各自还各自的。
    pub(super) retired: VecDeque<Arc<RenderImage>>,
    /// 鼠标是否停在小窗上。
    pub(super) hovered: bool,
    /// 悬浮条与控制栏是否可见。比 `hovered` 慢半拍，见 `set_hovered`。
    pub(super) controls: bool,
    /// 拖动进度条时的临时位置；`None` 表示没在拖。
    pub(super) dragging: Option<f64>,
    /// 拖动音量滑块时为真。
    pub(super) volume_dragging: bool,
    /// 音量滑块是否展开。
    pub(super) volume_open: bool,
    /// 指针停在音量图标上。
    pub(super) volume_hovered: bool,
    /// 指针停在音量面板上。图标与面板是两个独立的命中区域，少了这个标志，
    /// 指针一挪到面板上「离开图标」就会把面板收掉。
    pub(super) volume_panel_hovered: bool,
    /// 进度条与音量滑块的 bounds，由 canvas 在 prepaint 阶段写回。
    pub(super) progress_bounds: Rc<Cell<Bounds<Pixels>>>,
    pub(super) volume_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// 用户想不想让小窗置顶。写在偏好里，下次开小窗照它来。
    pub(super) on_top: bool,
    /// 已经下发给窗口系统的置顶状态；`None` 表示还没下发过。
    pub(super) on_top_applied: Option<bool>,
    /// 上一次记下的窗口宽度，用来判断用户有没有拖动过边框（变了才回写偏好）。
    pub(super) last_width: f32,
    /// 主窗口要求关掉小窗（或者是主窗口没了）。真正的关闭要等到下一次 render，
    /// 那时才拿得到 `&mut Window`。
    pub(super) dismissed: bool,
    /// 主窗口已经被释放，跟着退出。
    pub(super) app_gone: bool,
    pub(super) focus: FocusHandle,
}

impl MiniPlayer {
    pub(super) fn new(
        app: WeakEntity<PlayerApp>,
        state: MiniState,
        on_top: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            app,
            state,
            frame: None,
            incoming: None,
            retired: VecDeque::new(),
            hovered: false,
            controls: false,
            dragging: None,
            volume_dragging: false,
            volume_open: false,
            volume_hovered: false,
            volume_panel_hovered: false,
            progress_bounds: Rc::new(Cell::new(Bounds::default())),
            volume_bounds: Rc::new(Cell::new(Bounds::default())),
            on_top,
            on_top_applied: None,
            last_width: 0.0,
            dismissed: false,
            app_gone: false,
            focus: cx.focus_handle(),
        }
    }

    /// 换上刚到的那一帧，旧的排队等着还给 atlas。
    pub(super) fn swap_frame(&mut self, window: &mut Window) {
        let Some(incoming) = self.incoming.take() else {
            return;
        };
        if let Some(previous) = self.frame.replace(incoming) {
            self.retired.push_back(previous);
            while self.retired.len() > 2 {
                if let Some(stale) = self.retired.pop_front() {
                    let _ = window.drop_image(stale);
                }
            }
        }
    }

    /// 小窗一律置顶——不置顶它就被别的窗口盖住了，弹出来等于没弹。
    ///
    /// 与主窗口的 `sync_on_top` 各记各的：这是两个窗口，层级互不相关。
    pub(super) fn sync_on_top(&mut self, window: &Window, cx: &mut Context<Self>) {
        let want = self.on_top;
        if self.on_top_applied == Some(want) {
            return;
        }
        self.on_top_applied = Some(want);
        window.defer(cx, move |window, _cx| {
            if !platform::topmost::set(window, want) {
                log_warn!("小窗置顶未生效：当前窗口系统不支持");
            }
        });
    }

    /// 点到小窗就把它抬到普通窗口层的最前。
    ///
    /// 光靠系统默认的点击激活不够确定：被别的应用盖住时，激活并不总会把它抬到
    /// 那一层的最前，于是「点了它还是被压着」。这里显式改一次 Z 序。
    /// 置顶状态下不用管——它本来就在所有普通窗口之上。
    pub(super) fn raise_to_top(&self, window: &Window) {
        if self.on_top {
            return;
        }
        platform::topmost::raise(window);
    }

    /// 图钉按钮。切完立刻下发，不等下一次 render——置顶是个立刻能感觉到的动作。
    pub(super) fn toggle_on_top(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.on_top = !self.on_top;
        let want = self.on_top;
        // 顺手把 `on_top_applied` 记上，免得下一帧的 `sync_on_top` 又下发一遍。
        self.on_top_applied = Some(want);
        if !platform::topmost::set(window, want) {
            log_warn!("小窗置顶切换未生效：当前窗口系统不支持");
        }
        self.forward(cx, move |app, _cx| {
            app.settings.mini_on_top = want;
            app.settings.save();
        });
        cx.notify();
    }

    /// 从主窗口拉一份显示状态过来。由 `cx.observe` 在主窗口 notify 之后调用，
    /// 那时两边都没被借用，可以安全 `read`（原因见 `MiniState` 的文档）。
    pub(super) fn pull_state(&mut self, app: &Entity<PlayerApp>, cx: &mut Context<Self>) {
        let state = app.read(cx).mini_state();
        if self.state.same_as(&state) {
            return;
        }
        self.state = state;
        cx.notify();
    }

    /// 把动作转给主窗口：播放器状态只有一份，小窗自己不持有 mpv 句柄。
    pub(super) fn forward(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut PlayerApp, &mut Context<PlayerApp>),
    ) {
        if let Some(app) = self.app.upgrade() {
            app.update(cx, f);
        }
    }

    /// 尺寸变了就记进偏好，下次开小窗按用户拖过的宽度还原。
    ///
    /// 回写推到 defer 里而不是当场改：这段代码跑在 render 中，小窗的第一次绘制
    /// 又是从主窗口的回调里发起的，此刻 `PlayerApp` 正被可变借用，直接改它会 panic。
    pub(super) fn remember_width(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let width = f32::from(window.viewport_size().width);
        if (width - self.last_width).abs() < 1.0 {
            return;
        }
        self.last_width = width;
        let Some(app) = self.app.upgrade() else {
            return;
        };
        window.defer(cx, move |_window, cx| {
            app.update(cx, |app, _cx| app.settings.mini_width = width);
        });
    }

    /// 鼠标进出。控件不是跟着指针立刻闪，而是移开之后再等 500ms：
    /// 从画面挪到底部控制栏的无主区域、或者贴边划过，都不该让控件抖一下。
    pub(super) fn set_hovered(&mut self, hovered: bool, cx: &mut Context<Self>) {
        self.hovered = hovered;
        if hovered {
            if !self.controls {
                self.controls = true;
            }
            cx.notify();
            return;
        }

        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            this.update(cx, |mini, cx| {
                // 期间指针又回来了就什么都不做。
                if mini.hovered || !mini.controls {
                    return;
                }
                mini.controls = false;
                mini.volume_open = false;
                mini.volume_hovered = false;
                mini.volume_panel_hovered = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 指针横坐标换算成片内秒数。
    pub(super) fn time_for_x(&self, x: Pixels) -> f64 {
        let bounds = self.progress_bounds.get();
        let width = f32::from(bounds.size.width);
        if width <= 1.0 || self.state.duration <= 0.0 {
            return self.state.position;
        }
        let offset = f32::from(x) - f32::from(bounds.origin.x);
        (offset as f64 / width as f64).clamp(0.0, 1.0) * self.state.duration
    }

    /// 音量面板的显隐。图标与面板都算「焦点」，指针在这两者之间移动时中间
    /// 会经过一小段不属于任何一边的空白，所以「失去焦点」不是立刻收，
    /// 而是等 150ms——期间移回来就作罢。
    pub(super) fn refresh_volume_panel(&mut self, cx: &mut Context<Self>) {
        if self.volume_hovered || self.volume_panel_hovered {
            if !self.volume_open {
                self.volume_open = true;
                cx.notify();
            }
            return;
        }
        if !self.volume_open {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            this.update(cx, |mini, cx| {
                // 拖滑块时指针可能已经跑出轨道，这种情况不算失去焦点。
                if mini.volume_hovered || mini.volume_panel_hovered || mini.volume_dragging {
                    return;
                }
                mini.volume_open = false;
                mini.volume_panel_hovered = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 指针纵坐标换算成音量百分比。上限跟主窗口一致（mpv 能到 130）。
    ///
    /// 竖直滑块是「上大下小」，得翻过来算：`Pixels` 的 y 轴向下增长，
    /// 所以 y 越小音量越大。
    pub(super) fn volume_for_y(&self, y: Pixels) -> f64 {
        let bounds = self.volume_bounds.get();
        let height = f32::from(bounds.size.height);
        if height <= 1.0 {
            return self.state.volume;
        }
        let offset = f32::from(y) - f32::from(bounds.origin.y);
        let ratio = (offset / height).clamp(0.0, 1.0);
        (1.0 - ratio as f64) * 130.0
    }

    /// 顶部悬浮条：标题 + 置顶 + 还原 + 关闭。
    pub(super) fn render_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("mini-bar")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(32.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .px(px(8.0))
            .bg(ui::scrim())
            // 点悬浮条的空白处不该穿透到底下的画面上去触发「单击暂停」。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.raise_to_top(window);
                }),
            )
            .child(
                // 拖标题移动窗口。这里必须 stop_propagation，否则会被下面的画面
                // 当成一次单击而顺手把播放暂停掉。
                div()
                    .id("mini-title")
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(12.0))
                    .text_color(text())
                    .cursor(CursorStyle::PointingHand)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.raise_to_top(window);
                            window.start_window_move();
                        }),
                    )
                    .child(self.state.title.clone()),
            )
            .child(icon_button(
                "mini-on-top",
                // 两个图标而不是同一个图标变色：置顶与否只看颜色的差别太弱，
                // 尤其在视频画面上什么底色都可能出现。
                if self.on_top {
                    IconName::Pin
                } else {
                    IconName::PinOff
                },
                self.on_top,
                false,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.toggle_on_top(window, cx);
                }),
            ))
            .child(icon_button(
                "mini-restore",
                IconName::Minimize2,
                false,
                false,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    // 先把主窗口拉回来再关自己：反过来的话中间那几帧屏幕上什么都没有。
                    this.forward(cx, |app, cx| app.reactivate_main(cx));
                    window.remove_window();
                    cx.notify();
                }),
            ))
            .child(icon_button(
                "mini-close",
                IconName::X,
                false,
                false,
                // 悬浮条上的关闭和主窗口标题栏上的关闭是同一件事：结束整个应用。
                // 「回到大窗口」是左边那个还原按钮的职责，两者不能混。
                cx.listener(|_this, _: &ClickEvent, _window, cx| cx.quit()),
            ))
            .into_any_element()
    }

    /// 底部控制栏：进度 + 播放暂停 + 时间 + 音量。
    pub(super) fn render_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let duration = self.state.duration;
        let position = self.dragging.unwrap_or(self.state.position);
        let ratio = if duration > 0.0 {
            (position / duration).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let slot = self.progress_bounds.clone();

        div()
            .id("mini-controls")
            .absolute()
            .bottom_0()
            .left_0()
            .right_0()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .pt(px(6.0))
            .pb(px(8.0))
            .bg(ui::scrim())
            // 同上：控制栏上的空白也不该被当成「点画面」。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.raise_to_top(window);
                }),
            )
            .child(
                div()
                    .id("mini-progress")
                    .relative()
                    .w_full()
                    .h(px(14.0))
                    .flex()
                    .items_center()
                    .cursor(CursorStyle::PointingHand)
                    .child(
                        canvas(
                            move |bounds, _window, _cx| slot.set(bounds),
                            |_bounds, (), _window, _cx| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(
                        div()
                            .relative()
                            .h(px(4.0))
                            .w_full()
                            .rounded(px(2.0))
                            .bg(gpui_kit::rgba(0xffffff30))
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .bottom_0()
                                    .w(gpui_kit::relative(ratio))
                                    .rounded(px(2.0))
                                    .bg(ui::accent())
                                    .child(
                                        div()
                                            .absolute()
                                            .right(px(-4.0))
                                            .top(px(-3.0))
                                            .size(px(10.0))
                                            .rounded_full()
                                            .bg(ui::accent()),
                                    ),
                            ),
                    )
                    // 按下即定位、拖动跟随、松手才真正 seek：和主窗口一致，
                    // 边拖边 seek 在长片子上会把解码线程反复打断。
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            cx.stop_propagation();
                            let at = this.time_for_x(event.position.x);
                            this.dragging = Some(at);
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                        if this.dragging.is_none() {
                            return;
                        }
                        let at = this.time_for_x(event.position.x);
                        this.dragging = Some(at);
                        cx.notify();
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseUpEvent, _window, cx| {
                            let Some(position) = this.dragging.take() else {
                                return;
                            };
                            this.forward(cx, |app, cx| app.seek_to(position, cx));
                            cx.notify();
                        }),
                    ),
            )
            .child(
                // 进度条顶到窗口两边，这一行才留内边距：进度条的可用行程就是整个窗口宽。
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(10.0))
                    .child(icon_button(
                        "mini-play",
                        if self.state.playing {
                            IconName::Pause
                        } else {
                            IconName::Play
                        },
                        false,
                        false,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.forward(cx, |app, cx| app.toggle_play(cx));
                        }),
                    ))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text_muted())
                            .child(format!(
                                "{} / {}",
                                format_time(position),
                                format_time(duration)
                            )),
                    )
                    .child(div().flex_1())
                    .child(self.render_volume(cx)),
            )
            .into_any_element()
    }

    /// 音量：点图标切静音，悬停展开滑块。与主窗口的 `render_volume` 同一套手势。
    pub(super) fn render_volume(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = self.state.muted;
        let volume = if muted { 0.0 } else { self.state.volume };
        let ratio = (volume / 130.0).clamp(0.0, 1.0) as f32;
        let icon = if muted || volume <= 0.0 {
            IconName::VolumeX
        } else if volume < 55.0 {
            IconName::Volume1
        } else {
            IconName::Volume2
        };
        let slot = self.volume_bounds.clone();

        div()
            .id("mini-volume-wrap")
            .relative()
            .child(icon_button(
                "mini-volume",
                icon,
                muted,
                false,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.forward(cx, |app, cx| app.toggle_mute(cx));
                }),
            ))
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                this.volume_hovered = *hovered;
                this.refresh_volume_panel(cx);
            }))
            .when(self.volume_open, |this| {
                let fill = if muted { text_faint() } else { text() };
                this.child(
                    // 竖直面板浮在图标正上方。窗口只有几百像素宽，横向滑块会把
                    // 控制栏挤得没地方放时间，竖向也不占额外宽度。
                    div()
                        .id("mini-volume-slider")
                        .absolute()
                        .bottom(px(30.0))
                        // 面板比图标宽，往左挪半个差值让它压着图标居中。
                        .left(px(-13.0))
                        .w(px(44.0))
                        .py(px(10.0))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(px(8.0))
                        .bg(ui::scrim())
                        // 面板自己也算「焦点」，否则指针一离开图标就被当成失焦。
                        .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                            this.volume_panel_hovered = *hovered;
                            this.refresh_volume_panel(cx);
                        }))
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(if muted { text_faint() } else { text() })
                                .child(format!("{volume:.0}%")),
                        )
                        .child(
                            div()
                                .id("mini-volume-track")
                                .relative()
                                .w(px(20.0))
                                .h(px(88.0))
                                .flex()
                                .justify_center()
                                .child(
                                    canvas(
                                        move |bounds, _window, _cx| slot.set(bounds),
                                        |_bounds, (), _window, _cx| {},
                                    )
                                    .absolute()
                                    .inset_0(),
                                )
                                .child(
                                    div()
                                        .relative()
                                        .w(px(4.0))
                                        .h_full()
                                        .rounded(px(2.0))
                                        .bg(gpui_kit::rgba(0xffffff30))
                                        .child(
                                            // 从底往上长，滑块圆点挂在填充区顶端。
                                            div()
                                                .absolute()
                                                .bottom_0()
                                                .left_0()
                                                .right_0()
                                                .h(gpui_kit::relative(ratio))
                                                .rounded(px(2.0))
                                                .bg(fill)
                                                .child(
                                                    div()
                                                        .absolute()
                                                        .top(px(-5.0))
                                                        .left(px(-4.0))
                                                        .size(px(12.0))
                                                        .rounded_full()
                                                        .bg(fill),
                                                ),
                                        ),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                                        cx.stop_propagation();
                                        this.volume_dragging = true;
                                        let volume = this.volume_for_y(event.position.y);
                                        this.forward(cx, move |app, cx| {
                                            app.set_volume_from_ui(volume, cx)
                                        });
                                    }),
                                )
                                .on_mouse_move(cx.listener(
                                    |this, event: &MouseMoveEvent, _window, cx| {
                                        if !this.volume_dragging {
                                            return;
                                        }
                                        let volume = this.volume_for_y(event.position.y);
                                        this.forward(cx, move |app, cx| {
                                            app.set_volume_from_ui(volume, cx)
                                        });
                                    },
                                ))
                                .on_mouse_up(
                                    MouseButton::Left,
                                    cx.listener(|this, _: &MouseUpEvent, _window, cx| {
                                        this.volume_dragging = false;
                                        cx.notify();
                                    }),
                                ),
                        ),
                )
            })
            .into_any_element()
    }
}

impl Render for MiniPlayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 主窗口要求关掉，或者干脆已经被释放（用户把整个应用关了），
        // 这个小窗既没有控件也没有人驱动，跟着一起关，免得留下一个关不掉、
        // 也没法操作的空窗口。关闭放在这里而不是回调里：只有 render 拿得到
        // `&mut Window`。
        if self.dismissed || self.app_gone || self.app.upgrade().is_none() {
            window.remove_window();
            return div().size_full().bg(bg()).into_any_element();
        }

        self.swap_frame(window);
        self.sync_on_top(window, cx);
        self.remember_width(window, cx);

        // 这里不去读 `PlayerApp`：小窗的第一次绘制发生在主窗口的回调里，
        // 那时主窗口正被可变借用。标题和进度由观察回调提前拉进 `state`（见 `pull_state`）。
        let hint = if self.state.empty {
            "没有正在播放的文件"
        } else {
            "正在加载…"
        };

        div()
            .id("mini-root")
            .relative()
            .size_full()
            .bg(bg())
            .text_color(text())
            .track_focus(&self.focus)
            .child(ui::video_surface(self.frame.clone()))
            .when(self.frame.is_none(), |this| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(8.0))
                        .child(icon(IconName::Film, 34.0, text_faint()))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(text_muted())
                                .child(hint),
                        ),
                )
            })
            // 两条同进同出：分开触发的话鼠标在中间移来移去会让它们各闪各的。
            .when(self.controls, |this| {
                this.child(self.render_bar(cx))
                    .child(self.render_controls(cx))
            })
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                this.set_hovered(*hovered, cx);
            }))
            // 画面、按钮、音量面板的按下都会冒泡到这里（悬浮条与控制栏自己截住了，
            // 它们各自也调了 `raise_to_top`），所以这一个入口就覆盖了整窗。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, _cx| {
                    this.raise_to_top(window);
                }),
            )
            .on_click(cx.listener(|this, event: &ClickEvent, _window, cx| {
                // 双击的第二下不再触发切换，否则一次双击会连切两下、等于没按。
                if event.click_count() >= 2 {
                    return;
                }
                this.forward(cx, |app, cx| app.toggle_play(cx));
                cx.notify();
            }))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                // 只在音量面板展开时接管滚轮：那一刻指针必然停在图标或面板上，
                // 也就是「用户正在调音量」。其余位置滚轮不做任何事——小窗没有
                // 可滚动的内容，接过滚轮也不会抢走别处的手势。
                if !this.volume_open {
                    return;
                }
                let delta = match event.delta {
                    ScrollDelta::Pixels(delta) => f32::from(delta.y),
                    ScrollDelta::Lines(delta) => delta.y,
                };
                if delta.abs() < 0.01 {
                    return;
                }
                // 与主窗口一致：一格一步，不看增量大小。
                let step = if delta > 0.0 {
                    VOLUME_STEP
                } else {
                    -VOLUME_STEP
                };
                this.forward(cx, move |app, cx| app.change_volume(step, cx));
            }))
            .on_action(cx.listener(|this, _: &TogglePlay, _window, cx| {
                this.forward(cx, |app, cx| app.toggle_play(cx));
            }))
            .on_action(cx.listener(|this, _: &ToggleMute, _window, cx| {
                this.forward(cx, |app, cx| app.toggle_mute(cx));
            }))
            .on_action(cx.listener(|this, _: &SeekForward, _window, cx| {
                this.forward(cx, |app, cx| app.seek_by(SEEK_STEP, cx));
            }))
            .on_action(cx.listener(|this, _: &SeekBackward, _window, cx| {
                this.forward(cx, |app, cx| app.seek_by(-SEEK_STEP, cx));
            }))
            .on_action(cx.listener(|this, _: &VolumeUp, _window, cx| {
                this.forward(cx, |app, cx| app.change_volume(VOLUME_STEP, cx));
            }))
            .on_action(cx.listener(|this, _: &VolumeDown, _window, cx| {
                this.forward(cx, |app, cx| app.change_volume(-VOLUME_STEP, cx));
            }))
            .on_action(cx.listener(|this, _: &ToggleMini, window, cx| {
                // T 在小窗里的含义与主窗口一致——切换小窗模式，也就是还原。
                this.forward(cx, |app, cx| app.reactivate_main(cx));
                window.remove_window();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ExitOverlay, window, cx| {
                // 小窗里没有别的浮层可关，Esc 就是退出小窗模式。
                this.forward(cx, |app, cx| app.reactivate_main(cx));
                window.remove_window();
                cx.notify();
            }))
            .into_any_element()
    }
}
