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
    /// 进度条与音量滑块的 bounds，由 canvas 在 prepaint 阶段写回。
    pub(super) progress_bounds: Rc<Cell<Bounds<Pixels>>>,
    pub(super) volume_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// 已经下发给窗口系统的置顶状态。
    pub(super) on_top_applied: bool,
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
            progress_bounds: Rc::new(Cell::new(Bounds::default())),
            volume_bounds: Rc::new(Cell::new(Bounds::default())),
            on_top_applied: false,
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
        if self.on_top_applied {
            return;
        }
        self.on_top_applied = true;
        window.defer(cx, move |window, _cx| {
            if !platform::topmost::set(window, true) {
                log_warn!("小窗置顶未生效：当前窗口系统不支持");
            }
        });
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

    /// 指针横坐标换算成音量百分比。上限跟主窗口一致（mpv 能到 130）。
    pub(super) fn volume_for_x(&self, x: Pixels) -> f64 {
        let bounds = self.volume_bounds.get();
        let width = f32::from(bounds.size.width);
        if width <= 1.0 {
            return self.state.volume;
        }
        let offset = f32::from(x) - f32::from(bounds.origin.x);
        (offset as f64 / width as f64).clamp(0.0, 1.0) * 130.0
    }

    /// 顶部悬浮条：标题 + 还原 + 关闭。
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
                cx.listener(|_this, _: &MouseDownEvent, _window, cx| cx.stop_propagation()),
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
                        cx.listener(|_this, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            window.start_window_move();
                        }),
                    )
                    .child(self.state.title.clone()),
            )
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
            .px(px(10.0))
            .pt(px(6.0))
            .pb(px(8.0))
            .bg(ui::scrim())
            // 同上：控制栏上的空白也不该被当成「点画面」。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_this, _: &MouseDownEvent, _window, cx| cx.stop_propagation()),
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
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
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
            // 悬停只是「打开」，关上交给控件整体收起的那一刻：鼠标从图标挪到滑块上时
            // 图标必然先失去悬停，若在这里跟着关，滑块会瞬间消失，根本点不到。
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if *hovered && !this.volume_open {
                    this.volume_open = true;
                    cx.notify();
                }
            }))
            .when(self.volume_open, |this| {
                this.child(
                    div()
                        .id("mini-volume-slider")
                        .absolute()
                        .bottom(px(30.0))
                        .right_0()
                        .w(px(120.0))
                        .h(px(24.0))
                        .px(px(10.0))
                        .py(px(9.0))
                        .rounded(px(6.0))
                        .bg(ui::scrim())
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
                                        .bg(if muted { text_faint() } else { text() }),
                                )
                                .child(
                                    canvas(
                                        move |bounds, _window, _cx| slot.set(bounds),
                                        |_bounds, (), _window, _cx| {},
                                    )
                                    .absolute()
                                    .inset_0(),
                                ),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                                cx.stop_propagation();
                                this.volume_dragging = true;
                                let volume = this.volume_for_x(event.position.x);
                                this.forward(cx, move |app, cx| app.set_volume_from_ui(volume, cx));
                            }),
                        )
                        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                            if !this.volume_dragging {
                                return;
                            }
                            let volume = this.volume_for_x(event.position.x);
                            this.forward(cx, move |app, cx| app.set_volume_from_ui(volume, cx));
                        }))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _: &MouseUpEvent, _window, cx| {
                                this.volume_dragging = false;
                                cx.notify();
                            }),
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

        // 这里不再去读 `PlayerApp`：小窗的第一次绘制发生在主窗口的回调里，
        // 那时主窗口正被可变借用。标题和进度都由主窗口推送过来（见 `PlayerApp::sync_mini`）。
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
            .on_click(cx.listener(|this, event: &ClickEvent, _window, cx| {
                // 双击的第二下不再触发切换，否则一次双击会连切两下、等于没按。
                if event.click_count() >= 2 {
                    return;
                }
                this.forward(cx, |app, cx| app.toggle_play(cx));
                cx.notify();
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
