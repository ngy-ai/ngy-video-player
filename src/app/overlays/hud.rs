//! 播放统计浮层与 OSD：调试信息、左上角提示。

use crate::app::*;

impl PlayerApp {
    /// 播放统计浮层：帧率与三种丢帧计数。按 I 开关，右上角常驻。
    ///
    /// 只在出问题时才有看头，所以默认关着；数字非零才染成警示色，正常播放时
    /// 整块是灰的，不会抢画面注意力。
    pub(in crate::app) fn render_stats_card(
        &self,
        top: Pixels,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.show_stats {
            return None;
        }
        let snapshot = &self.snapshot;
        let resolution = if snapshot.has_video() {
            format!("{}×{}", snapshot.video_width, snapshot.video_height)
        } else {
            "无画面".to_string()
        };
        let fps = if snapshot.fps > 0.05 {
            format!("{:.2} fps", snapshot.fps)
        } else {
            "—".to_string()
        };

        Some(
            div()
                .id("stats-card")
                .absolute()
                .right(px(24.0))
                .top(top)
                .w(px(196.0))
                .p(px(10.0))
                .rounded(px(10.0))
                .bg(surface())
                .border_1()
                .border_color(gpui_kit::rgb(0x2c2c34))
                .shadow_lg()
                .flex()
                .flex_col()
                .gap(px(3.0))
                // 卡片浮在画面上：点击与按下都要吃掉，否则会漏到底下切播放状态。
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_this, _: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                    }),
                )
                .child(
                    div()
                        .pb(px(2.0))
                        .text_size(px(10.0))
                        .text_color(text_faint())
                        .child("播放统计"),
                )
                .child(stat_row("分辨率", resolution, false))
                .child(stat_row("帧率", fps, false))
                .child(stat_row(
                    "丢帧",
                    snapshot.frame_drops.to_string(),
                    snapshot.frame_drops > 0,
                ))
                .child(stat_row(
                    "延迟帧",
                    snapshot.mistimed_frames.to_string(),
                    snapshot.mistimed_frames > 0,
                ))
                .child(stat_row(
                    "解码丢帧",
                    snapshot.decoder_drops.to_string(),
                    snapshot.decoder_drops > 0,
                ))
                .child(
                    div()
                        .pt(px(2.0))
                        .text_size(px(9.0))
                        .text_color(text_faint())
                        .child("按 I 关闭"),
                )
                .into_any_element(),
        )
    }

    pub(in crate::app) fn render_osd(&self, top: Pixels, panel: f32) -> Option<impl IntoElement> {
        let (lines, _) = self.osd.as_ref()?;
        Some(
            div()
                .absolute()
                .left(px(0.0))
                // 提示条在画面上居中，别压到侧边栏上。
                .right(px(panel))
                .top(top)
                .flex()
                .justify_center()
                .child(
                    div()
                        .px(px(14.0))
                        .py(px(8.0))
                        .rounded(px(10.0))
                        .bg(gpui_kit::rgba(0x000000cc))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(2.0))
                        .children(lines.iter().map(|line| {
                            div()
                                .text_size(px(14.0))
                                .text_color(text())
                                .child(line.clone())
                        })),
                ),
        )
    }
}

/// 统计浮层里的一行：左边标题、右边数值，非零时染警示色。
fn stat_row(label: &'static str, value: String, danger: bool) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.0))
        .child(
            div()
                .text_size(px(11.0))
                .text_color(text_muted())
                .child(label),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(if danger { ui::danger() } else { text() })
                .child(value),
        )
        .into_any_element()
}
