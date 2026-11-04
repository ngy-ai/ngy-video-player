//! 小窗的整窗组装：自毁判定、帧交换、标题与输入分发。

use super::*;

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

        // 任务栏按钮上写的就是窗口标题，小窗现在承担那一条（见 `open_mini` 的
        // `kind`），所以标题得跟着片子走。变了才下发：`SetWindowText` 每次都
        // 会通知外壳，每帧一次纯属浪费。
        let title = if self.state.empty {
            "小窗".to_string()
        } else {
            format!("{} · 小窗", self.state.title)
        };
        if self.window_title != title {
            self.window_title = title.clone();
            window.set_window_title(&title);
        }

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
            .on_action(cx.listener(|this, _: &ToggleStats, _window, cx| {
                this.forward(cx, |app, cx| {
                    app.show_stats ^= true;
                    cx.notify();
                });
            }))
            .on_action(cx.listener(|this, _: &SubtitleEarlier, _window, cx| {
                this.forward(cx, |app, cx| app.set_subtitle_delay(-0.1, cx));
            }))
            .on_action(cx.listener(|this, _: &SubtitleLater, _window, cx| {
                this.forward(cx, |app, cx| app.set_subtitle_delay(0.1, cx));
            }))
            .on_action(cx.listener(|this, _: &SeekForward, _window, cx| {
                this.forward(cx, |app, cx| app.seek_by(SEEK_STEP, cx));
            }))
            .on_action(cx.listener(|this, _: &SeekBackward, _window, cx| {
                this.forward(cx, |app, cx| app.seek_by(-SEEK_STEP, cx));
            }))
            // 逐帧在主窗口与小窗里是同一套键（Ctrl/⌘ + ←/→），这里不接的话
            // 小窗里按下去毫无反应——动作是全局键位表派发的，但得有元素接。
            .on_action(cx.listener(|this, _: &StepBackward, _window, cx| {
                this.forward(cx, |app, cx| app.step_frame(-1, cx));
            }))
            .on_action(cx.listener(|this, _: &StepForward, _window, cx| {
                this.forward(cx, |app, cx| app.step_frame(1, cx));
            }))
            .on_action(cx.listener(|this, _: &VolumeUp, _window, cx| {
                this.forward(cx, |app, cx| app.change_volume(VOLUME_STEP, cx));
            }))
            .on_action(cx.listener(|this, _: &VolumeDown, _window, cx| {
                this.forward(cx, |app, cx| app.change_volume(-VOLUME_STEP, cx));
            }))
            .on_action(cx.listener(|this, _: &ToggleMini, window, cx| {
                // T 在小窗里的含义与主窗口一致——切换小窗模式，也就是还原。
                this.dismissed = true;
                this.forward(cx, |app, cx| app.reactivate_main(cx));
                window.remove_window();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ExitOverlay, window, cx| {
                // 小窗里没有别的浮层可关，Esc 就是退出小窗模式。
                this.dismissed = true;
                this.forward(cx, |app, cx| app.reactivate_main(cx));
                window.remove_window();
                cx.notify();
            }))
            .into_any_element()
    }
}
