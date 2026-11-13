//! 变速面板里的速度曲线画布：网格、折线、可拖的倍率点与播放头。
//!
//! 横轴是**素材内偏移**（相对源入点，秒），纵轴是倍率；倍率取 log2，0.25 / 0.5 / 1 / 2 / 4
//! 才均匀分布，1× 正好在中间。时间线上的曲线标记共用 [`rate_ratio`]，两处高低含义一致。

use super::*;

impl PlayerApp {
    /// 速度曲线画布：网格 + 刻度 + 折线 + 可拖的点 + 播放头竖线。
    pub(super) fn render_speed_curve(&self, clip: Id, cx: &mut Context<Self>) -> AnyElement {
        let Some(project) = self.project.as_ref() else {
            return div().into_any_element();
        };
        let Some(item) = project.sequence().clip(clip) else {
            return div().into_any_element();
        };
        let span = item.span().secs().max(0.001);
        let fallback = item.rate();
        let playhead = project.ui.playhead;
        let offset = source_offset_at_playhead(item, playhead);
        let points: Vec<crate::timeline::SpeedPoint> = item.remap.points.clone();

        let mut canvas = div()
            .id("speed-curve-canvas")
            .relative()
            .w(px(CURVE_WIDTH))
            .h(px(CURVE_HEIGHT))
            .rounded(px(6.0))
            .bg(gpui_kit::rgb(0x14141a))
            .border_1()
            .border_color(ui::track())
            .overflow_hidden();

        // 横线：0.5× / 1× / 2× 三档参考，1× 加亮——它才是「原速」。
        for (rate, label) in [(0.5f32, "0.5×"), (1.0, "1×"), (2.0, "2×")] {
            let y = curve_y(rate);
            let strong = (rate - 1.0).abs() < 1e-6;
            canvas = canvas.child(
                div()
                    .absolute()
                    .top(px(y - 0.5))
                    .left(px(CURVE_PADDING))
                    .right(px(CURVE_PADDING))
                    .h(px(1.0))
                    .bg(if strong {
                        gpui_kit::rgba(0xffffff28)
                    } else {
                        gpui_kit::rgba(0xffffff12)
                    }),
            );
            canvas = canvas.child(
                div()
                    .absolute()
                    .right(px(2.0))
                    .top(px((y - 6.0).max(0.0)))
                    .text_size(px(8.0))
                    .text_color(text_faint())
                    .child(label),
            );
        }
        // 竖线：四等分素材跨度，标上秒数。
        for step in 0..=4 {
            let ratio = step as f32 / 4.0;
            let x = CURVE_PADDING + ratio * (CURVE_WIDTH - CURVE_PADDING * 2.0);
            canvas = canvas.child(
                div()
                    .absolute()
                    .left(px(x - 0.5))
                    .top(px(CURVE_PADDING))
                    .bottom(px(CURVE_PADDING))
                    .w(px(1.0))
                    .bg(gpui_kit::rgba(0xffffff10)),
            );
            if step % 2 == 0 {
                canvas = canvas.child(
                    div()
                        .absolute()
                        .left(px((x - 12.0).max(1.0)))
                        .bottom(px(0.0))
                        .text_size(px(8.0))
                        .text_color(text_faint())
                        .child(format!("{:.1}s", span * f64::from(ratio))),
                );
            }
        }

        // 折线：没点时是一条水平线（常量倍率），有点时按曲线采样。
        let mut previous: Option<(f32, f32)> = None;
        for index in 0..CURVE_SAMPLES {
            let at = span * (index as f64 / (CURVE_SAMPLES - 1) as f64);
            let rate = item.remap.rate_at(at, fallback);
            let x = offset_x(at, span);
            let y = curve_y(rate);
            if let Some((px0, py0)) = previous {
                let (left, width) = (px0.min(x), (x - px0).abs().max(1.0));
                let (top, height) = (py0.min(y), (y - py0).abs().max(1.5));
                canvas = canvas.child(
                    div()
                        .absolute()
                        .left(px(left))
                        .top(px(top))
                        .w(px(width))
                        .h(px(height))
                        .bg(gpui_kit::rgb(0xffd166)),
                );
            }
            previous = Some((x, y));
        }

        // 播放头：告诉用户「记点」会落在曲线的哪个位置。
        let head_x = offset_x(offset, span);
        canvas = canvas.child(
            div()
                .absolute()
                .left(px(head_x - 0.5))
                .top(px(2.0))
                .bottom(px(2.0))
                .w(px(1.0))
                .bg(ui::accent()),
        );

        // 曲线上的点：可以拖（横拖改位置、纵拖改倍率）。
        for (index, point) in points.iter().enumerate() {
            let (x, y) = (offset_x(point.at, span), curve_y(point.rate));
            let point_rate = point.rate;
            let active = self.speed_drop.is_some_and(|drop| {
                drop.clip == clip && drop.index == index && cx.has_active_drag()
            });
            let ink: Hsla = if active {
                ui::accent()
            } else {
                gpui_kit::rgb(0xffd166).into()
            };
            canvas = canvas.child(
                div()
                    .id(("speed-point", index))
                    .absolute()
                    .left(px(x - 4.0))
                    .top(px(y - 4.0))
                    .w(px(8.0))
                    .h(px(8.0))
                    .rounded(px(2.0))
                    .bg(ink)
                    .border_1()
                    .border_color(gpui_kit::rgba(0x00000088))
                    .cursor(CursorStyle::PointingHand)
                    .on_drag(
                        SpeedDrag { clip, index },
                        move |_drag, _offset, _window, cx| {
                            cx.new(|_| super::timeline_tracks::TimelineGhost {
                                name: SharedString::from(format!("{point_rate:.2}×")),
                            })
                        },
                    ),
            );
        }

        // 拖动中的点画大一点：松手前就能看出会落到哪个倍率。
        if let Some(drop) = self.speed_drop
            && drop.clip == clip
            && cx.has_active_drag()
        {
            let (x, y) = (offset_x(drop.at, span), curve_y(drop.rate));
            canvas = canvas
                .child(
                    div()
                        .absolute()
                        .left(px(x - 2.0))
                        .top(px(CURVE_PADDING))
                        .bottom(px(CURVE_PADDING))
                        .w(px(1.5))
                        .bg(ui::accent()),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(x + 4.0))
                        .top(px((y - 14.0).max(1.0)))
                        .px(px(4.0))
                        .rounded(px(3.0))
                        .bg(gpui_kit::rgba(0x1b1b24f0))
                        .text_size(px(9.0))
                        .text_color(ui::accent())
                        .child(format!("{:.2}×", drop.rate)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(x - 5.0))
                        .top(px(y - 5.0))
                        .w(px(10.0))
                        .h(px(10.0))
                        .rounded(px(3.0))
                        .bg(ui::accent()),
                );
        }

        // 拖点：落点在拖动过程中算好，松手才落一步撤销（与曲线编辑器同一套写法）。
        canvas = canvas
            .on_drag_move(cx.listener(
                move |this, event: &gpui_kit::DragMoveEvent<SpeedDrag>, _window, cx| {
                    let position = event.event.position;
                    if !event.bounds.contains(&position) {
                        return;
                    }
                    let (drag_clip, index) = {
                        let drag = event.drag(cx);
                        (drag.clip, drag.index)
                    };
                    let Some(project) = this.project.as_ref() else {
                        return;
                    };
                    let Some(item) = project.sequence().clip(drag_clip) else {
                        return;
                    };
                    let span = item.span().secs().max(0.001);
                    let x = f32::from(position.x) - f32::from(event.bounds.origin.x);
                    let y = f32::from(position.y) - f32::from(event.bounds.origin.y);
                    let (at, rate) = to_curve(x, y, span);
                    this.speed_drop = Some(SpeedDrop {
                        clip: drag_clip,
                        index,
                        at,
                        rate,
                    });
                    cx.notify();
                },
            ))
            // 落点必须属于这次拖的那个点：上次拖到一半松手（没落在画布上）会留下
            // 一个旧落点，不核对一下就会把旧值套到这次的点上。
            .on_drop(cx.listener(move |this, drag: &SpeedDrag, _window, cx| {
                let matched = this
                    .speed_drop
                    .is_some_and(|drop| drop.clip == drag.clip && drop.index == drag.index);
                if matched {
                    this.drop_speed_point(cx);
                } else {
                    this.speed_drop = None;
                    cx.notify();
                }
            }));

        // 「在播放头记点」：倍率取曲线当前值，所以记下去的那一刻不会有变化，
        // 接下来拖它才是有意义的操作。
        let record = div()
            .id("speed-curve-record")
            .px(px(8.0))
            .h(px(20.0))
            .flex()
            .items_center()
            .rounded(px(4.0))
            .bg(gpui_kit::rgba(0xffffff10))
            .text_size(px(10.0))
            .text_color(text_muted())
            .cursor(CursorStyle::PointingHand)
            .child("在播放头记点")
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                cx.stop_propagation();
                let Some(project) = this.project.as_ref() else {
                    return;
                };
                let Some(item) = project.sequence().clip(clip) else {
                    return;
                };
                let at = source_offset_at_playhead(item, project.ui.playhead);
                let rate = item.remap.rate_at(at, item.rate());
                this.set_speed_point(clip, at, rate, cx);
            }));

        let clear = div()
            .id("speed-curve-clear")
            .px(px(8.0))
            .h(px(20.0))
            .flex()
            .items_center()
            .rounded(px(4.0))
            .bg(gpui_kit::rgba(0xffffff10))
            .text_size(px(10.0))
            .text_color(if points.is_empty() {
                text_faint()
            } else {
                text_muted()
            })
            .cursor(CursorStyle::PointingHand)
            .child("清空曲线")
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                cx.stop_propagation();
                this.clear_speed_curve(clip, cx);
            }));

        let hint = if points.is_empty() {
            "还没有曲线：记一个点，再拖它调倍率".to_string()
        } else {
            format!(
                "{} 个点 · 平均 {:.2}×",
                points.len(),
                span / duration_seconds(item)
            )
        };

        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(canvas)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(record)
                    .child(clear)
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(9.0))
                            .text_color(text_faint())
                            .child(hint),
                    ),
            )
            .into_any_element()
    }
}

/// 算「平均倍率」用：片段时长（0 时按 1 秒兜底，免得除零）。
pub(super) fn duration_seconds(item: &ClipItem) -> f64 {
    (item.motion().secs()).max(0.001)
}

/// 素材内偏移 → 画布横坐标。
fn offset_x(at: f64, span: f64) -> f32 {
    let ratio = (at / span.max(0.001)).clamp(0.0, 1.0) as f32;
    CURVE_PADDING + ratio * (CURVE_WIDTH - CURVE_PADDING * 2.0)
}

/// 倍率 → 画布纵坐标（上边是 4×，下边是 0.25×）。
fn curve_y(rate: f32) -> f32 {
    let ratio = rate_ratio(rate);
    CURVE_PADDING + (1.0 - ratio) * (CURVE_HEIGHT - CURVE_PADDING * 2.0)
}

/// 倍率在量程里的位置（0 = 0.25×，1 = 4×，1× 正好在 0.5）。
///
/// 取 log2：线性轴会把 1× 挤在底部，四档倍率看起来有三档挤在一起。
/// 时间线上的曲线标记也用这个函数，两处的「高低」含义一致。
pub(in crate::app) fn rate_ratio(rate: f32) -> f32 {
    let rate = if rate.is_finite() && rate > 0.0 {
        rate.clamp(ClipItem::MIN_SPEED, ClipItem::MAX_SPEED)
    } else {
        1.0
    };
    ((rate.log2() + 2.0) / 4.0).clamp(0.0, 1.0)
}

/// 画布纵坐标 → 倍率（`rate_ratio` 的反函数），拖动手柄时用。
fn y_rate(y: f32) -> f32 {
    let ratio = 1.0 - (y - CURVE_PADDING) / (CURVE_HEIGHT - CURVE_PADDING * 2.0);
    2f32.powf(ratio.clamp(0.0, 1.0) * 4.0 - 2.0)
        .clamp(ClipItem::MIN_SPEED, ClipItem::MAX_SPEED)
}

/// 画布坐标 → (素材内偏移秒, 倍率)。拖点的时候把鼠标位置换回曲线坐标。
fn to_curve(x: f32, y: f32, span: f64) -> (f64, f32) {
    let width = CURVE_WIDTH - CURVE_PADDING * 2.0;
    let ratio = ((x - CURVE_PADDING) / width).clamp(0.0, 1.0);
    ((ratio as f64 * span).clamp(0.0, span), y_rate(y))
}

/// 播放头在片段内走到素材的哪个位置（速度曲线的自变量）。
///
/// 片段之外的播放头贴到两端：面板上的「记点」永远有一个落点，不会因为播放头在
/// 片段外面而落空。这里走 `source_at`，所以有曲线时它反解的是**曲线**上的位置。
pub(super) fn source_offset_at_playhead(item: &ClipItem, playhead: Tick) -> f64 {
    let span = item.span().secs();
    let offset = playhead - item.start;
    if offset <= Tick::ZERO {
        return 0.0;
    }
    if offset >= item.motion() {
        return span;
    }
    item.source_at(playhead)
        .map(|source| (source - item.source_in).secs())
        .unwrap_or(0.0)
        .clamp(0.0, span)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn 倍率画到对数轴上时原速在中间() {
        assert!((rate_ratio(1.0) - 0.5).abs() < 1e-6, "1× 在正中间");
        assert!((rate_ratio(0.25) - 0.0).abs() < 1e-6, "下限贴底");
        assert!((rate_ratio(4.0) - 1.0).abs() < 1e-6, "上限贴顶");
        // 单调：倍率越大画得越高（`rate_ratio` 越大）。
        let mut previous = -1.0;
        for rate in [0.25f32, 0.5, 1.0, 1.5, 2.0, 4.0] {
            let ratio = rate_ratio(rate);
            assert!(ratio > previous, "{rate} 的位置没往上走");
            previous = ratio;
        }
        // 坏值按原速画，别让一个 NaN 把折线画到画布外面去。
        assert!((rate_ratio(f32::NAN) - 0.5).abs() < 1e-6);
        assert!((rate_ratio(-1.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn 画布坐标与倍率可以互逆() {
        for rate in [0.25f32, 0.5, 1.0, 2.0, 4.0] {
            let y = curve_y(rate);
            let back = y_rate(y);
            assert!((back - rate).abs() < 0.01, "{rate} → {y} → {back}");
        }
        // 横轴：跨度的两端落在内边距上，中间是跨度的一半。
        assert!((offset_x(0.0, 4.0) - CURVE_PADDING).abs() < 1e-6);
        assert!((offset_x(4.0, 4.0) - (CURVE_WIDTH - CURVE_PADDING)).abs() < 1e-6);
        let (at, rate) = to_curve(offset_x(2.0, 4.0), curve_y(2.0), 4.0);
        assert!((at - 2.0).abs() < 0.01, "{at}");
        assert!((rate - 2.0).abs() < 0.01, "{rate}");
    }

    #[test]
    fn 播放头在片段外时记点贴到两端() {
        let mut clip = ClipItem::new(
            Id::new(1),
            crate::timeline::AssetId::new(1),
            Tick::from_secs(10.0),
            Tick::from_secs(14.0),
            Tick::from_secs(3.0),
        );
        let start = clip.start;
        assert_eq!(
            source_offset_at_playhead(&clip, start - Tick::from_secs(1.0)),
            0.0
        );
        assert_eq!(
            source_offset_at_playhead(&clip, start + Tick::from_secs(9.0)),
            4.0,
            "越过片段终点时贴到素材末尾"
        );
        assert!(
            (source_offset_at_playhead(&clip, start + Tick::from_secs(1.0)) - 1.0).abs() < 1e-6
        );

        // 有曲线时取的是曲线反解出来的素材位置：全程 2× 的片段，时间线 1 秒 = 素材 2 秒。
        clip.remap.insert(0.0, 2.0);
        let offset = source_offset_at_playhead(&clip, start + Tick::from_secs(1.0));
        assert!((offset - 2.0).abs() < 0.01, "{offset}");
    }
}
