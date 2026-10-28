//! 画面菜单与倍速面板。

use super::*;

impl PlayerApp {
    pub(super) fn render_menu(&self, panel: f32, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu?;
        // 自定义倍速是一块独立面板（标题栏 + 滑块），不套列表布局。
        if menu == Menu::SpeedCustom {
            return Some(self.render_speed_panel(panel, cx));
        }
        let mut entries: Vec<(String, bool, Option<MenuAction>)> = Vec::new();

        match menu {
            Menu::Speed => {
                for speed in SPEED_PRESETS {
                    entries.push((
                        format_speed(speed),
                        (self.snapshot.speed - speed).abs() < 0.001,
                        Some(MenuAction::Command(Command::SetSpeed(speed))),
                    ));
                }
                // 用键盘调出来的档位（如 1.15×）不在预设里，直接在这一行标出来，
                // 否则打开菜单看到的是一排都没选中的数字。
                let custom = !SPEED_PRESETS
                    .iter()
                    .any(|preset| (self.snapshot.speed - preset).abs() < 0.001);
                entries.push((
                    if custom {
                        format!("自定义倍数 · {}", format_speed_value(self.snapshot.speed))
                    } else {
                        "自定义倍数…".to_string()
                    },
                    custom,
                    Some(MenuAction::SpeedCustom),
                ));
            }
            // 上面已经单独返回，这里不需要内容。
            Menu::SpeedCustom => {}
            Menu::Loop => {
                for mode in [LoopMode::Off, LoopMode::All, LoopMode::One] {
                    entries.push((
                        mode.label().to_string(),
                        self.settings.loop_mode == mode,
                        Some(MenuAction::Loop(mode)),
                    ));
                }
            }
            Menu::Subtitle => {
                entries.push((
                    "关闭字幕".to_string(),
                    self.snapshot
                        .selected_track(crate::player::TrackKind::Subtitle)
                        .is_none(),
                    Some(MenuAction::Command(Command::SelectTrack {
                        kind: crate::player::TrackKind::Subtitle,
                        id: -1,
                    })),
                ));
                for track in self.snapshot.track(crate::player::TrackKind::Subtitle) {
                    entries.push((
                        track.display_name(),
                        track.selected,
                        Some(MenuAction::Command(Command::SelectTrack {
                            kind: crate::player::TrackKind::Subtitle,
                            id: track.id,
                        })),
                    ));
                }
            }
            Menu::Audio => {
                for track in self.snapshot.track(crate::player::TrackKind::Audio) {
                    entries.push((
                        track.display_name(),
                        track.selected,
                        Some(MenuAction::Command(Command::SelectTrack {
                            kind: crate::player::TrackKind::Audio,
                            id: track.id,
                        })),
                    ));
                }
                if entries.is_empty() {
                    entries.push(("没有可切换的音轨".to_string(), false, None));
                }
            }
        }

        Some(
            div()
                .id("menu")
                .absolute()
                // 跟着控制条一起让开面板，否则弹出菜单会飘到侧边栏上。
                .right(px(18.0 + panel))
                .bottom(px(74.0))
                .w(px(220.0))
                .py(px(6.0))
                .rounded(px(10.0))
                .bg(surface())
                .border_1()
                .border_color(gpui_kit::rgb(0x2c2c34))
                .shadow_lg()
                // 菜单项自己不管冒泡：这一层统一把点击吃掉，否则选个倍速/字幕
                // 会连带把点击漏给画面，顺带切一次播放状态。
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .children(entries.into_iter().enumerate().map(
                    |(index, (label, active, action))| {
                        let id = gpui_kit::ElementId::Name(format!("menu-{index}").into());
                        let enabled = action.is_some();
                        div()
                            .id(id)
                            .px(px(10.0))
                            .py(px(7.0))
                            .mx(px(4.0))
                            .rounded(px(6.0))
                            .text_size(px(13.0))
                            .text_color(if active {
                                ui::accent()
                            } else if enabled {
                                text()
                            } else {
                                text_faint()
                            })
                            .when(enabled, |this| {
                                this.cursor(CursorStyle::PointingHand)
                                    .hover(|style| style.bg(ui::surface_hover()))
                            })
                            .child(label)
                            .when_some(action, |this, action| {
                                this.on_click(cx.listener(
                                    move |this, _: &ClickEvent, _window, cx| {
                                        match action.clone() {
                                            MenuAction::Command(command) => {
                                                this.send(command);
                                                this.menu = None;
                                            }
                                            MenuAction::Loop(mode) => {
                                                this.set_loop(mode, cx);
                                                this.menu = None;
                                            }
                                            // 自定义面板是倍速菜单的第二层，不关菜单只换内容。
                                            MenuAction::SpeedCustom => {
                                                this.menu = Some(Menu::SpeedCustom);
                                            }
                                        }
                                        this.poke();
                                        cx.notify();
                                    },
                                ))
                            })
                    },
                ))
                // 字幕菜单额外提供一个「从文件加载」入口。
                .when(menu == Menu::Subtitle, |this| {
                    this.child(
                        div()
                            .h(px(1.0))
                            .mx(px(8.0))
                            .my(px(4.0))
                            .bg(gpui_kit::rgb(0x2c2c34)),
                    )
                    .child(
                        div()
                            .id("menu-load-sub")
                            .px(px(10.0))
                            .py(px(7.0))
                            .mx(px(4.0))
                            .rounded(px(6.0))
                            .text_size(px(13.0))
                            .text_color(text_muted())
                            .cursor(CursorStyle::PointingHand)
                            .hover(|style| style.bg(ui::surface_hover()))
                            .child("从文件加载字幕…")
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.menu = None;
                                this.pick_subtitle(window, cx);
                            })),
                    )
                })
                .into_any_element(),
        )
    }

    /// 自定义倍速面板：标题栏 + 跟随滑块的气泡 + 0.1–5.0 的滑杆。
    ///
    /// 气泡和滑块头的位置都用固定宽度算，而不是读 canvas 回填的 bounds：bounds
    /// 要到 prepaint 之后才有值，拿它排版会让气泡慢一帧。canvas 那份只负责把
    /// 真实命中区域写回 `speed_bounds`，供拖动时换算坐标。
    pub(super) fn render_speed_panel(&self, panel: f32, cx: &mut Context<Self>) -> AnyElement {
        const WIDTH: f32 = 264.0;
        const PAD: f32 = 14.0;
        const LABEL: f32 = 22.0;
        const GAP: f32 = 8.0;
        const BUBBLE: f32 = 56.0;
        const TRACK: f32 = WIDTH - PAD * 2.0 - (LABEL + GAP) * 2.0;

        let speed = self.snapshot.speed.clamp(SPEED_MIN, SPEED_MAX);
        let ratio = ((speed - SPEED_MIN) / (SPEED_MAX - SPEED_MIN)).clamp(0.0, 1.0) as f32;
        let dragging = self.speed_dragging;
        // 气泡中心跟着滑块头走，靠近两端时收在面板内。
        let bubble_left =
            (PAD + LABEL + GAP + ratio * TRACK - BUBBLE / 2.0).clamp(PAD, WIDTH - PAD - BUBBLE);
        let knob = if dragging { 14.0 } else { 12.0 };
        let slot = self.speed_bounds.clone();

        div()
            .id("speed-menu")
            .absolute()
            .right(px(18.0 + panel))
            .bottom(px(74.0))
            .w(px(WIDTH))
            .py(px(6.0))
            .rounded(px(10.0))
            .bg(surface())
            .border_1()
            .border_color(gpui_kit::rgb(0x2c2c34))
            .shadow_lg()
            // 和列表菜单一样挡住点击，否则点面板空白处会穿透到画面切一次播放。
            .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                cx.stop_propagation();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(10.0))
                    .py(px(4.0))
                    .child(
                        div()
                            .id("speed-back")
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(px(24.0))
                            .rounded(px(6.0))
                            .cursor(CursorStyle::PointingHand)
                            .hover(|style| style.bg(ui::surface_hover()))
                            .child(icon(IconName::ChevronLeft, 16.0, text()))
                            .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                                this.menu = Some(Menu::Speed);
                                this.poke();
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(text())
                            .child("自定义倍数"),
                    ),
            )
            .child(
                div()
                    .h(px(1.0))
                    .mx(px(10.0))
                    .my(px(4.0))
                    .bg(gpui_kit::rgb(0x2c2c34)),
            )
            .child(
                div().relative().h(px(30.0)).w_full().child(
                    div()
                        .absolute()
                        .left(px(bubble_left))
                        .top(px(2.0))
                        .w(px(BUBBLE))
                        .h(px(24.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.0))
                        .bg(ui::surface_hover())
                        .text_size(px(12.0))
                        .text_color(text())
                        .child(format_speed_value(speed)),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(GAP))
                    .px(px(PAD))
                    .pb(px(8.0))
                    .child(speed_edge_label("0.1", LABEL))
                    .child(
                        div()
                            .id("speed-slider")
                            .relative()
                            .flex_1()
                            .h(px(20.0))
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
                                    .h(px(3.0))
                                    .w_full()
                                    .rounded(px(2.0))
                                    .bg(ui::track())
                                    .child(
                                        div()
                                            .absolute()
                                            .left_0()
                                            .top_0()
                                            .bottom_0()
                                            .w(gpui_kit::relative(ratio))
                                            .rounded(px(2.0))
                                            .bg(text())
                                            .child(
                                                div()
                                                    .absolute()
                                                    .right(px(-knob / 2.0))
                                                    .top(px(-(knob - 3.0) / 2.0))
                                                    .size(px(knob))
                                                    .rounded_full()
                                                    .bg(text()),
                                            ),
                                    ),
                            )
                            // 刻度只标整数档：全量程铺满反而看不清滑块在哪一档。
                            .children((1..=4).map(|tick| {
                                let at = (f64::from(tick) - SPEED_MIN) / (SPEED_MAX - SPEED_MIN);
                                div()
                                    .absolute()
                                    .left(gpui_kit::relative(at as f32))
                                    .top(px(8.0))
                                    .w(px(1.0))
                                    .h(px(4.0))
                                    .bg(text_faint())
                            }))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                    this.speed_dragging = true;
                                    this.set_speed_from_x(event.position.x, cx);
                                    // 按下时即使数值没变也要重画：滑块头要放大一圈。
                                    cx.notify();
                                }),
                            )
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener(|this, _: &MouseUpEvent, _window, cx| {
                                    this.speed_dragging = false;
                                    // 松手才落盘：拖动途中每移动一像素写一次太浪费。
                                    this.settings.save();
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(speed_edge_label("5.0", LABEL)),
            )
            .into_any_element()
    }
}
