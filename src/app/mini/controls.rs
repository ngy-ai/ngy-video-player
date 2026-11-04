//! 小窗底部控制栏：进度、播放暂停、时间与音量面板。

use super::*;

impl MiniPlayer {
    /// 松开进度条：把拖到的位置真正 seek 过去。
    pub(super) fn end_progress_drag(&mut self, cx: &mut Context<Self>) {
        let Some(position) = self.dragging.take() else {
            return;
        };
        self.forward(cx, |app, cx| app.seek_to(position, cx));
        cx.notify();
    }

    /// 指针横坐标换算成片内秒数。算不出来时保持当前位置。
    pub(super) fn time_for_x(&self, x: Pixels) -> f64 {
        geometry::time_for_x(self.progress_bounds.get(), x, self.state.duration)
            .unwrap_or(self.state.position)
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

    /// 指针纵坐标换算成音量百分比。算不出来时保持当前音量。
    pub(super) fn volume_for_y(&self, y: Pixels) -> f64 {
        geometry::volume_for_y(self.volume_bounds.get(), y).unwrap_or(self.state.volume)
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
                            this.end_progress_drag(cx)
                        }),
                    )
                    // 小窗又矮又窄，拖进度条时指针很容易跑到窗外；那种情况下抬起
                    // 落在空的命中区里，上面的 `on_mouse_up` 收不到。和主窗口一样
                    // 补一条「抬起在本元素之外」的路径。
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseUpEvent, _window, cx| {
                            this.end_progress_drag(cx)
                        }),
                    ),
            )
            .child(
                // 进度条顶到窗口两边，这一行才留内边距：进度条的可用行程就是整个窗口宽。
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(10.0))
                    .child(icon_button_with_tip(
                        "mini-prev",
                        IconName::SkipBack,
                        false,
                        self.state.items <= 1,
                        Some("上一集（P）"),
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.forward(cx, |app, cx| app.previous_file(cx));
                        }),
                    ))
                    .child(icon_button_with_tip(
                        "mini-play",
                        if self.state.playing {
                            IconName::Pause
                        } else {
                            IconName::Play
                        },
                        false,
                        false,
                        Some(if self.state.playing {
                            "暂停（空格）"
                        } else {
                            "播放（空格）"
                        }),
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.forward(cx, |app, cx| app.toggle_play(cx));
                        }),
                    ))
                    .child(icon_button_with_tip(
                        "mini-next",
                        IconName::SkipForward,
                        false,
                        self.state.items <= 1,
                        Some("下一集（N）"),
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.forward(cx, |app, cx| app.next_file(cx));
                        }),
                    ))
                    .child(icon_button_with_tip(
                        "mini-subtitle",
                        IconName::Captions,
                        self.state.subtitle,
                        !self.state.has_subtitle,
                        Some(if self.state.subtitle {
                            "关闭字幕"
                        } else {
                            "字幕"
                        }),
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.forward(cx, |app, cx| {
                                let kind = TrackKind::Subtitle;
                                // 已经开着就关掉；没开就选第一条字幕轨。
                                let id = if app.snapshot.selected_track(kind).is_some() {
                                    -1
                                } else {
                                    match app.snapshot.track(kind).first() {
                                        Some(track) => track.id,
                                        None => return,
                                    }
                                };
                                app.send(Command::SelectTrack { kind, id });
                                cx.notify();
                            });
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
            .child(icon_button_with_tip(
                "mini-volume",
                icon,
                muted,
                false,
                // 这颗按钮只切静音，调音量是悬停后弹出的竖条；提示得说清这一点，
                // 否则会让人以为点一下能调大小。
                Some(if muted {
                    "取消静音（M）·悬停调音量"
                } else {
                    "静音（M）·悬停调音量"
                }),
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
                                        // 拖动期间 `pull_state` 不浮音量，松手补一条最终值。
                                        let text = if this.state.muted {
                                            "已静音".to_string()
                                        } else {
                                            format!("音量 {:.0}%", this.state.volume)
                                        };
                                        this.flash_text(SharedString::from(text), cx);
                                        cx.notify();
                                    }),
                                ),
                        ),
                )
            })
            .into_any_element()
    }
}
