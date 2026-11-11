//! 时间线的轨道区：按垂直滚动窗口画轨道行，并接管这一块上的滚轮。
//!
//! 标题条与标尺固定在上方，轨道区独占剩下的高度。以前这里只画「前几条」，
//! 加轨按钮把新轨追加到序列末尾后就永远露不出来；现在按 `ViewState` 给出的
//! 可见区间画，加轨后自动跟到最后一屏（见 `ViewState::sync_tracks`）。
//!
//! 滚轮分工：轨道区里上下滚 = 换轨道，按住 Shift = 横向滚时间轴，
//! `Ctrl/⌘` = 以鼠标位置为锚点缩放；触控板的横向滑动仍直接滚时间轴。

use super::*;

impl PlayerApp {
    /// 画轨道区（不含标题条、标尺与播放头）。`top` 坐标都相对这块容器。
    pub(in crate::app) fn render_timeline_tracks_area(
        &self,
        height: f32,
        lane_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(project) = &self.project else {
            return div().into_any_element();
        };
        let sequence = project.sequence();
        let tracks_top = TIMELINE_HEADER + TIMELINE_RULER;
        let area_height = (height - tracks_top).max(0.0);
        let row_px = TIMELINE_TRACK_HEIGHT + TIMELINE_TRACK_GAP;
        let rows = visible_track_rows(height);
        let total = sequence.tracks.len();
        let range = self.timeline.visible_tracks(total, rows);
        let first = range.start;

        let mut area = div()
            .id("timeline-tracks")
            .absolute()
            .top(px(tracks_top))
            .left_0()
            .right_0()
            .bottom_0()
            .on_scroll(cx.listener(|this, event: &ScrollEvent, window, cx| {
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
                // 不截断的话根时间线会再拿 dy 横滚一次，画面会同时往两个方向跑。
                cx.stop_propagation();
                if event.modifiers.secondary(cx) {
                    let factor = if dy > 0.0 { 1.25 } else { 0.8 };
                    let anchor = f32::from(event.position.x) - TIMELINE_GUTTER;
                    this.zoom_timeline(factor, anchor);
                } else if event.modifiers.shift {
                    // 普通鼠标只有 y：按住 Shift 时把它借给时间轴。
                    this.scroll_timeline(-(if dx.abs() > 0.01 { dx } else { dy }));
                } else {
                    if dx.abs() > 0.01 {
                        this.scroll_timeline(-dx);
                    }
                    if dy.abs() > 0.01 {
                        let count = this
                            .project
                            .as_ref()
                            .map_or(0, |project| project.sequence().tracks.len());
                        let visible = visible_track_rows(this.timeline_height(window));
                        let row_px = TIMELINE_TRACK_HEIGHT + TIMELINE_TRACK_GAP;
                        let moved = this
                            .timeline
                            .scroll_tracks_by_wheel(dy, row_px, count, visible);
                        // 轨道没溢出时让滚轮回到老本行：横向滚时间轴，别让滚轮突然失灵。
                        if !moved && count <= visible {
                            this.scroll_timeline(-dy);
                        }
                    }
                }
                cx.notify();
            }));

        for (row, track) in sequence.tracks[range].iter().enumerate() {
            let top = row as f32 * row_px;
            area = area
                .child(self.render_timeline_lane(project, track, top, lane_width, cx))
                .child(self.render_timeline_label(track, top, cx));
        }

        if total == 0 {
            area = area.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(TIMELINE_TRACK_HEIGHT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.0))
                    .text_color(text_faint())
                    .child("这条序列还没有轨道"),
            );
        }

        // 轨道多到一屏放不下时给一条能点的滚动条：既是位置提示，也解释为什么
        // 加轨之后最上面几条不见了。滚动全靠滚轮也行，但「还有多少轨」得看得见。
        if total > rows {
            let max_scroll = total - rows;
            let thumb_h = (area_height * rows as f32 / total as f32)
                .max(18.0)
                .min(area_height.max(18.0));
            let thumb_top = (area_height - thumb_h) * first as f32 / max_scroll as f32;
            area = area.child(
                div()
                    .id("timeline-track-scroll")
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right_0()
                    .w(px(6.0))
                    .bg(gpui_kit::rgba(0x00000044))
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(px(thumb_top))
                            .h(px(thumb_h))
                            .rounded(px(3.0))
                            .bg(gpui_kit::rgba(0x9a9aaccc)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            let panel_height = this.timeline_height(window);
                            let area_top = f32::from(window.viewport_size().height) - panel_height
                                + tracks_top;
                            let y = f32::from(event.position.y) - area_top;
                            let fraction = (y / area_height.max(1.0)).clamp(0.0, 1.0);
                            let count = this
                                .project
                                .as_ref()
                                .map_or(0, |project| project.sequence().tracks.len());
                            let visible = visible_track_rows(panel_height);
                            let target = (fraction * count.saturating_sub(visible) as f32).round();
                            this.timeline
                                .scroll_tracks_to(target as usize, count, visible);
                            cx.notify();
                        }),
                    ),
            );
        }

        area.into_any_element()
    }
}
