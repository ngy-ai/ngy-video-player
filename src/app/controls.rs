//! 底部控制条与音量面板。

use super::*;

impl PlayerApp {
    pub(super) fn render_volume(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = self.snapshot.muted;
        let volume = if muted { 0.0 } else { self.snapshot.volume };
        let ratio = (volume / 130.0).clamp(0.0, 1.0) as f32;
        let slot = self.volume_bounds.clone();

        div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .child(icon_button_with_tip(
                "mute",
                if muted || volume <= 0.0 {
                    IconName::VolumeX
                } else if volume < 55.0 {
                    IconName::Volume1
                } else {
                    IconName::Volume2
                },
                muted,
                false,
                // 提示跟着当前状态走：已经静音时写「取消静音」，否则容易让人以为
                // 再点一下会继续静音。
                Some(if muted {
                    "取消静音（M）"
                } else {
                    "静音（M）"
                }),
                cx.listener(|this, _: &ClickEvent, _window, cx| this.toggle_mute(cx)),
            ))
            .child(
                div()
                    .id("volume")
                    .relative()
                    .w(px(84.0))
                    .h(px(18.0))
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
                            .bg(gpui_kit::rgb(0x3a3a44))
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .bottom_0()
                                    .w(gpui_kit::relative(ratio))
                                    .rounded(px(2.0))
                                    .bg(if muted { text_faint() } else { text() }),
                            ),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            cx.stop_propagation();
                            this.volume_dragging = true;
                            this.set_volume_from_x(event.position.x);
                            cx.notify();
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseUpEvent, _window, cx| {
                            this.volume_dragging = false;
                            this.settings.volume = this.snapshot.volume;
                            cx.notify();
                        }),
                    ),
            )
    }

    pub(super) fn set_volume_from_x(&mut self, x: Pixels) {
        let bounds = self.volume_bounds.get();
        let width = f32::from(bounds.size.width) as f64;
        if width <= 1.0 {
            return;
        }
        let offset = f32::from(x) - f32::from(bounds.origin.x);
        let ratio = (offset as f64 / width).clamp(0.0, 1.0);
        let volume = ratio * 130.0;
        if self.snapshot.muted && volume > 0.0 {
            self.snapshot.muted = false;
            self.send(Command::SetMuted(false));
        }
        self.set_volume(volume);
    }

    pub(super) fn render_controls(&self, panel: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = &self.snapshot;
        let playing = !snapshot.paused;
        let position = self.progress_value();
        let duration = snapshot.duration;
        let has_video = snapshot.has_video();

        let time_text = if duration > 0.0 {
            format!("{} / {}", format_time(position), format_time(duration))
        } else {
            format!("{} / --:--", format_time(position))
        };

        let subtitle_active = snapshot
            .selected_track(crate::player::TrackKind::Subtitle)
            .is_some();

        div()
            .id("controls")
            .absolute()
            .left_0()
            .right(px(panel))
            .bottom_0()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .px(px(18.0))
            .pt(px(22.0))
            .pb(px(10.0))
            // 由下往上淡出，而不是一条硬边：画面与控件的交界不该像被裁过。
            .bg(gpui_kit::linear_gradient(
                0.0,
                gpui_kit::linear_color_stop(gpui_kit::hsla(0.0, 0.0, 0.0, 0.92), 0.0),
                gpui_kit::linear_color_stop(gpui_kit::hsla(0.0, 0.0, 0.0, 0.0), 1.0),
            ))
            .on_mouse_move(cx.listener(|this, _: &MouseMoveEvent, _window, cx| {
                this.poke();
                cx.notify();
            }))
            // 控制条是一整条面板：点在按钮之间的空白处不应该穿透到画面去
            // 触发「单击暂停」。按钮自己已经 stop_propagation，这里兜住剩下的。
            .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                cx.stop_propagation();
            }))
            .child(self.render_progress(cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    // 左：播放控制（提示里带上快捷键，和菜单栏的口径一致）
                    .child(icon_button_with_tip(
                        "prev",
                        IconName::SkipBack,
                        false,
                        self.playlist().len() <= 1,
                        Some("上一集（N）"),
                        cx.listener(|this, _: &ClickEvent, _window, cx| this.previous_file(cx)),
                    ))
                    .child(icon_button_with_tip(
                        "play",
                        if playing {
                            IconName::Pause
                        } else {
                            IconName::Play
                        },
                        false,
                        false,
                        Some(if playing {
                            "暂停（空格）"
                        } else {
                            "播放（空格）"
                        }),
                        cx.listener(|this, _: &ClickEvent, _window, cx| this.toggle_play(cx)),
                    ))
                    .child(icon_button_with_tip(
                        "next",
                        IconName::SkipForward,
                        false,
                        self.playlist().len() <= 1,
                        Some("下一集（P）"),
                        cx.listener(|this, _: &ClickEvent, _window, cx| this.next_file(cx)),
                    ))
                    .child(div().w(px(6.0)))
                    .child(self.render_volume(cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .ml(px(8.0))
                            .text_size(px(12.0))
                            .text_color(text_muted())
                            .child(div().child(time_text)),
                    )
                    .child(div().flex_1())
                    // 右：功能开关。这一排全是纯图标/当前值，鼠标停上去要有说明，
                    // 否则「字幕」和「音轨」、迷你窗和全屏只能靠试。
                    .child(pill_button_with_tip(
                        "speed",
                        IconName::Gauge,
                        format_speed(snapshot.speed),
                        self.menu.is_some_and(Menu::is_speed)
                            || (snapshot.speed - 1.0).abs() > 0.001,
                        Some("播放速度"),
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            // 预设列表和自定义面板是同一层，展开状态下再点一次直接收起。
                            this.menu = if this.menu.is_some_and(Menu::is_speed) {
                                None
                            } else {
                                Some(Menu::Speed)
                            };
                            this.menu_bar = None;
                            this.poke();
                            cx.notify();
                        }),
                    ))
                    .child(icon_button_with_tip(
                        "subtitle",
                        IconName::Captions,
                        subtitle_active,
                        false,
                        Some("字幕"),
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.menu =
                                (this.menu != Some(Menu::Subtitle)).then_some(Menu::Subtitle);
                            this.menu_bar = None;
                            this.poke();
                            cx.notify();
                        }),
                    ))
                    .child(icon_button_with_tip(
                        "audio",
                        IconName::AudioLines,
                        false,
                        !has_video,
                        Some("音轨"),
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.menu = (this.menu != Some(Menu::Audio)).then_some(Menu::Audio);
                            this.menu_bar = None;
                            this.poke();
                            cx.notify();
                        }),
                    ))
                    .child(pill_button_with_tip(
                        "loop",
                        match self.settings.loop_mode {
                            LoopMode::Off | LoopMode::All => IconName::Repeat,
                            LoopMode::One => IconName::Repeat1,
                        },
                        self.settings.loop_mode.short_label(),
                        self.menu == Some(Menu::Loop) || self.settings.loop_mode != LoopMode::Off,
                        Some("循环模式"),
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.menu = (this.menu != Some(Menu::Loop)).then_some(Menu::Loop);
                            this.menu_bar = None;
                            this.poke();
                            cx.notify();
                        }),
                    ))
                    .child(icon_button_with_tip(
                        "playlist-toggle",
                        IconName::ListVideo,
                        self.settings.show_playlist,
                        false,
                        Some("播放列表（L）"),
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.set_playlist_visible(!this.settings.show_playlist);
                            this.poke();
                            cx.notify();
                        }),
                    ))
                    .child(icon_button_with_tip(
                        "mini",
                        IconName::PictureInPicture2,
                        self.mini.is_some(),
                        false,
                        Some("迷你窗口（T）"),
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.toggle_mini(window, cx);
                        }),
                    ))
                    .child(icon_button_with_tip(
                        "fullscreen",
                        IconName::Maximize,
                        false,
                        false,
                        Some("全屏（F）"),
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.toggle_fullscreen(window);
                            cx.notify();
                        }),
                    )),
            )
    }
}
