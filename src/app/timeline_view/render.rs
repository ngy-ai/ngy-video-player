//! 时间线的整块绘制：标题条、标尺、轨道区与底部提示。

use super::*;

impl PlayerApp {
    // -- 渲染 --------------------------------------------------------------

    pub(in crate::app) fn render_timeline(
        &self,
        window: &Window,
        height: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(project) = &self.project else {
            return div().into_any_element();
        };
        let sequence = project.sequence();
        let view = &self.timeline;
        let window_width = f32::from(window.viewport_size().width);
        let lane_width = (window_width - TIMELINE_GUTTER).max(1.0);
        let playhead = project.ui.playhead.secs();

        let mut root = div()
            .id("timeline")
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(height))
            .overflow_hidden()
            .bg(surface())
            .border_t_1()
            .border_color(gpui_kit::rgb(0x26262e))
            // 面板上的点击不该漏到画面去（画面那一层负责播放/暂停）。
            .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                cx.stop_propagation();
            }))
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _window, cx| {
                if this.project.is_none() {
                    return;
                }
                let (dx, dy) = match event.delta {
                    ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y)),
                    ScrollDelta::Lines(delta) => (delta.x * 24.0, delta.y * 24.0),
                };
                if dx.abs() < 0.01 && dy.abs() < 0.01 {
                    return;
                }
                cx.stop_propagation();
                if event.modifiers.secondary(cx) {
                    // 上滚放大、下滚缩小；锚点是鼠标所在的轨道区横坐标。
                    let factor = if dy > 0.0 { 1.25 } else { 0.8 };
                    let anchor = f32::from(event.position.x) - TIMELINE_GUTTER;
                    this.zoom_timeline(factor, anchor);
                } else {
                    // 触控板横向滑动优先用 x；普通滚轮只有 y，也拿它横着滚。
                    let pixels = if dx.abs() > 0.01 { -dx } else { -dy };
                    this.scroll_timeline(pixels);
                }
                cx.notify();
            }));

        // 标题条：序列名、帧率、总时长、播放头时间与当前缩放。
        root = root.child(self.render_timeline_header(cx));

        // 标尺：主刻度带文字，之间四分之一的短刻度。
        let mut ruler = div()
            .absolute()
            .top(px(TIMELINE_HEADER))
            .left(px(TIMELINE_GUTTER))
            .right_0()
            .h(px(TIMELINE_RULER))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.timeline_dragging = true;
                    this.seek_timeline_to(event.position.x, event.modifiers.shift, cx);
                    this.poke();
                    cx.notify();
                }),
            )
            // 右键不看播放头在哪儿：工作区的清除入口得有一个位置无关的兜底。
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.open_ruler_menu(event.position, cx);
                }),
            );

        let step = view.tick_step();
        let minor_step = step / 5.0;
        // 一帧放得够宽就把刻度铺到帧：帧级对位不用再靠「猜半帧」。
        let frame_mode = frame_pixels(view, sequence.fps) >= f64::from(FRAME_TICK_PX);
        for tick in visible_ticks(view) {
            let x = view.x_of(tick);
            if x < -1.0 || x > lane_width {
                continue;
            }
            ruler = ruler
                .child(
                    div()
                        .absolute()
                        .left(px(x))
                        .top_0()
                        .bottom_0()
                        .w(px(1.0))
                        .bg(gpui_kit::rgb(0x3a3a44)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(x + 4.0))
                        .top(px(3.0))
                        .text_size(px(10.0))
                        .text_color(text_faint())
                        .child(format_time(tick)),
                );
            // 帧级时刻沿用秒级长线当骨架，短刻度交给下面的帧刻度；
            // 两套短刻度叠在一起会糊成一片。
            if frame_mode {
                continue;
            }
            for minor in 1..5 {
                let at = tick + minor_step * minor as f64;
                let minor_x = view.x_of(at);
                if minor_x < 0.0 || minor_x > lane_width {
                    continue;
                }
                ruler = ruler.child(
                    div()
                        .absolute()
                        .left(px(minor_x))
                        .bottom_0()
                        .h(px(5.0))
                        .w(px(1.0))
                        .bg(gpui_kit::rgb(0x2c2c34)),
                );
            }
        }

        if frame_mode {
            // 每帧一条，每 5 帧抬高一点当粗读单位；整秒那一根已经有长线了，
            // 这里再画一条短的也无妨——它正好落在长线里面，看不出重影。
            for frame in visible_frames(view, sequence.fps) {
                let at = Tick::from_frames(frame, sequence.fps);
                let x = view.x_of(at.secs());
                if x < 0.0 || x > lane_width {
                    continue;
                }
                // 高度都留在工作区色带（标尺上方 18–24）以下，不互相盖。
                let fifth = frame % 5 == 0;
                ruler = ruler.child(
                    div()
                        .absolute()
                        .left(px(x))
                        .bottom_0()
                        .h(px(if fifth { 8.0 } else { 5.0 }))
                        .w(px(1.0))
                        .bg(gpui_kit::rgb(if fifth { 0x3a3a44 } else { 0x2c2c34 })),
                );
            }
        }
        // 工作区：标尺下沿一条色带。标记竖线盖在它上面，两者互不遮挡。
        if let Some((start, end)) = project.ui.range() {
            let left = view.x_of(start.secs()).max(0.0);
            let right = view.x_of(end.secs()).min(lane_width);
            if right > left {
                ruler = ruler.child(
                    div()
                        .absolute()
                        .left(px(left))
                        .top(px(18.0))
                        .h(px(6.0))
                        .w(px(right - left))
                        .rounded(px(2.0))
                        .bg(gpui_kit::rgba(0x5aa9e6aa)),
                );
            }
        }

        // 工作区两端：只有一端时也必须画出来。色带要两端齐全才成立，只设入点时
        // 界面上原本什么变化都没有，用户既以为快捷键没生效，也没法把播放头拖回来。
        for (at, label) in [(project.ui.in_point, "入"), (project.ui.out_point, "出")] {
            let Some(at) = at else {
                continue;
            };
            let x = view.x_of(at.secs());
            if x < 0.0 || x > lane_width {
                continue;
            }
            ruler = ruler
                .child(
                    div()
                        .absolute()
                        .left(px(x))
                        .top_0()
                        .bottom_0()
                        .w(px(1.0))
                        .bg(gpui_kit::rgba(0x5aa9e6cc)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px((x + 3.0).min((lane_width - 10.0).max(0.0))))
                        .top_0()
                        .text_size(px(9.0))
                        .text_color(gpui_kit::rgb(0x5aa9e6))
                        .child(label),
                );
        }

        // 标记：贯穿标尺的竖线加顶上一颗小方块。名字不上标尺——刻度文字已经够挤了。
        for marker in &sequence.markers {
            let x = view.x_of(marker.at.secs());
            if x < 0.0 || x > lane_width {
                continue;
            }
            ruler = ruler
                .child(
                    div()
                        .absolute()
                        .left(px(x))
                        .top_0()
                        .bottom_0()
                        .w(px(1.0))
                        .bg(gpui_kit::rgba(0xf0c67499)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(x - 3.0))
                        .top_0()
                        .size(px(6.0))
                        .rounded(px(1.0))
                        .bg(gpui_kit::rgba(0xf0c674)),
                );
        }
        root = root.child(ruler);

        // 轨道区自己管垂直滚动：只画当前窗口里的那几条，新加的轨道会自动跟进来。
        root = root.child(self.render_timeline_tracks_area(height, lane_width, cx));

        // 播放头：贯穿标尺与轨道区。滚出视口就不画，免得贴在边上装成「在 0 秒」。
        let playhead_x = TIMELINE_GUTTER + view.x_of(playhead);
        if playhead_x >= TIMELINE_GUTTER && playhead_x <= window_width {
            root = root
                .child(
                    div()
                        .absolute()
                        .left(px(playhead_x - 1.0))
                        .top(px(TIMELINE_HEADER))
                        .bottom_0()
                        .w(px(2.0))
                        .bg(ui::accent()),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(playhead_x - 4.0))
                        .top(px(TIMELINE_HEADER - 3.0))
                        .size(px(9.0))
                        .rounded_full()
                        .bg(ui::accent()),
                );

            // 拖动时把精确时间贴在播放头旁边：标题条在窗口右端，盯着标尺的眼睛
            // 看不到；只有整秒的读数也没法对准 8.00 秒。
            if self.timeline_dragging {
                let bubble_w = 112.0;
                let left = if playhead_x + 8.0 + bubble_w <= window_width {
                    playhead_x + 8.0
                } else {
                    (playhead_x - 8.0 - bubble_w).max(TIMELINE_GUTTER)
                };
                let label = if self.timeline_snap {
                    format!("{} ⇧0.01s", format_time_precise(playhead))
                } else {
                    format_time_precise(playhead)
                };
                root = root.child(
                    div()
                        .absolute()
                        .left(px(left))
                        .top(px(TIMELINE_HEADER + 3.0))
                        .w(px(bubble_w))
                        .h(px(18.0))
                        .px(px(6.0))
                        .flex()
                        .items_center()
                        .rounded(px(3.0))
                        .bg(if self.timeline_snap {
                            gpui_kit::rgba(0x5aa9ff33)
                        } else {
                            gpui_kit::rgba(0x1b1b24f2)
                        })
                        .text_size(px(10.0))
                        .text_color(ui::accent())
                        .child(label),
                );
            }
        }

        // 双画面修剪窗口浮在最上面：它只在刚修剪完时出现，点一下就走。
        root = root.child(self.render_trim_window(cx));

        root.into_any_element()
    }
}
