//! 关键帧曲线编辑器（M10-T10）：把一条曲线画出来，拖贝塞尔控制点。
//!
//! 它挂在「关键帧」小面板里，不另开窗口：编辑曲线时眼睛要同时看时刻、属性值和曲线，
//! 分两个浮层反而要来回对。
//!
//! 坐标约定与曲线模型一致：横轴是**片段内秒数**，纵轴是属性取值（已经按属性的
//! `min`–`max` 归一化）。编辑的是「播放头所在那一段」——段由相邻两个关键帧定义，
//! 缓动存在起点那个点上（`Keyframe::easing`），所以拖的手柄属于起点。

use super::*;
use crate::timeline::{EASING_PRESETS, Id, Interpolation, Property, Tick};

/// 画布尺寸。宽度跟着面板走，高度固定：再高就挡画面了。
const CANVAS_WIDTH: f32 = 300.0;
const CANVAS_HEIGHT: f32 = 116.0;
const PADDING: f32 = 6.0;
const SAMPLES: usize = 60;

/// 正在拖的贝塞尔手柄。
///
/// 手柄没有走「点一下设值」：曲线的手感来自连续拖动，点一下只能摆到格子上。
pub(crate) struct CurveHandleDrag {
    pub(crate) clip: Id,
    pub(crate) property: Property,
    /// 第几个关键帧（它到下一个点这一段）。
    pub(crate) segment: usize,
    /// 1 = 第一个控制点，2 = 第二个。
    pub(crate) which: u8,
}

/// 在画布上拖一个框选范围。
pub(crate) struct CurveSelectDrag {
    pub(crate) clip: Id,
    pub(crate) property: Property,
}

/// 框选的起止点（曲线坐标 0–1）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CurveSelect {
    pub(crate) clip: Id,
    pub(crate) property: Property,
    pub(crate) from: (f32, f32),
    pub(crate) to: (f32, f32),
}

impl CurveSelect {
    fn contains(&self, at: f32, value: f32) -> bool {
        let (x0, x1) = (self.from.0.min(self.to.0), self.from.0.max(self.to.0));
        let (y0, y1) = (self.from.1.min(self.to.1), self.from.1.max(self.to.1));
        (x0..=x1).contains(&at) && (y0..=y1).contains(&value)
    }
}

/// 拖手柄时算出来的落点：曲线坐标（0–1，已经夹好）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CurveDrop {
    pub(crate) clip: Id,
    pub(crate) property: Property,
    pub(crate) segment: usize,
    pub(crate) which: u8,
    pub(crate) control: (f32, f32),
}

/// 一秒换多少像素：宽度固定时按片段时长摊。
fn x_of(at: Tick, duration: Tick) -> f32 {
    let span = duration.secs().max(0.001);
    PADDING + (at.secs() / span).clamp(0.0, 1.0) as f32 * (CANVAS_WIDTH - PADDING * 2.0)
}

fn y_of(value: f32, property: Property) -> f32 {
    let (min, max) = (property.min(), property.max());
    let span = (max - min).max(0.0001);
    let ratio = ((value - min) / span).clamp(0.0, 1.0);
    PADDING + (1.0 - ratio) * (CANVAS_HEIGHT - PADDING * 2.0)
}

/// 画布坐标 → 曲线坐标（0–1）。拖手柄时把鼠标位置换回控制点。
fn to_curve(x: f32, y: f32) -> (f32, f32) {
    let width = CANVAS_WIDTH - PADDING * 2.0;
    let height = CANVAS_HEIGHT - PADDING * 2.0;
    (
        ((x - PADDING) / width).clamp(0.0, 1.0),
        (1.0 - (y - PADDING) / height).clamp(0.0, 1.0),
    )
}

impl PlayerApp {
    /// 播放头落在第几段上。0 段 = 第一个点到第二个点。
    fn curve_segment(keys: &[crate::timeline::Keyframe], at: Tick) -> usize {
        keys.partition_point(|key| key.at <= at)
            .saturating_sub(1)
            .min(keys.len().saturating_sub(2))
    }

    /// 曲线编辑器整块：属性切换 + 画布 + 预设。
    pub(super) fn render_curve_editor(
        &self,
        clip: Id,
        property: Property,
        offset: Tick,
        duration: Tick,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(project) = self.project.as_ref() else {
            return div().into_any_element();
        };
        let Some(item) = project.sequence().clip(clip) else {
            return div().into_any_element();
        };
        let keys = &item.keys;
        let curve = keys.curve(property);
        let points = curve.map(|curve| curve.keys.as_slice()).unwrap_or_default();
        let fallback = curve.map(|curve| curve.interpolation).unwrap_or_default();

        let mut block = div().flex().flex_col().gap(px(4.0));

        // 属性切换：只在这块里切，不动面板下面那些行。
        let mut tabs = div().flex().items_center().gap(px(3.0)).child(
            div()
                .text_size(px(10.0))
                .text_color(text_faint())
                .child("曲线"),
        );
        for (index, option) in Property::ALL.into_iter().enumerate() {
            let animated = keys
                .curve(option)
                .is_some_and(|curve| !curve.keys.is_empty());
            tabs = tabs.child(chip(
                CURVE_TAB_IDS[index],
                option.label(),
                option == property,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    if let Some(TimelineMenu::Keys(menu)) = &mut this.timeline_menu {
                        menu.property = option;
                    }
                    cx.notify();
                }),
            ));
            if animated {
                tabs = tabs.child(
                    div()
                        .w(px(3.0))
                        .h(px(3.0))
                        .rounded(px(2.0))
                        .bg(ui::accent())
                        .ml(px(-3.0)),
                );
            }
        }
        block = block.child(tabs);

        block = block.child(self.render_curve_canvas(clip, property, offset, duration, cx));

        // 段级缓动：预设按钮 + 当前值。播放头所在段没有下一段时只有提示。
        if points.len() >= 2 {
            let segment = Self::curve_segment(points, offset);
            let current = points[segment].easing_or(fallback);
            let mut presets = div().flex().items_center().gap(px(3.0)).child(
                div()
                    .text_size(px(10.0))
                    .text_color(text_faint())
                    .child("缓动"),
            );
            for (index, (label, preset)) in EASING_PRESETS.into_iter().enumerate() {
                presets = presets.child(chip(
                    CURVE_PRESET_IDS[index],
                    label,
                    same_easing(current, preset),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.edit_keys(clip, cx, |sequence| {
                            let item = sequence.clip_mut(clip)?;
                            let curve = item.keys.ensure(property);
                            curve.keys.get_mut(segment)?.easing = Some(preset);
                            Some(format!(
                                "{}：第 {} 段缓动 {}",
                                property.label(),
                                segment + 1,
                                label
                            ))
                        });
                    }),
                ));
            }
            block = block.child(presets);
        }
        block = block.child(self.render_curve_clipboard(clip, property, cx));

        block.into_any_element()
    }

    /// 画布本体：网格 + 刻度 + 折线 + 关键帧 + 贝塞尔手柄。
    fn render_curve_canvas(
        &self,
        clip: Id,
        property: Property,
        offset: Tick,
        duration: Tick,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(project) = self.project.as_ref() else {
            return div().into_any_element();
        };
        let Some(item) = project.sequence().clip(clip) else {
            return div().into_any_element();
        };
        let curve = item.keys.curve(property);
        let points: Vec<crate::timeline::Keyframe> =
            curve.map(|curve| curve.keys.clone()).unwrap_or_default();
        let fallback = curve.map(|curve| curve.interpolation).unwrap_or_default();
        let segment = if points.len() >= 2 {
            Some(Self::curve_segment(&points, offset))
        } else {
            None
        };

        let mut canvas = div()
            .id("curve-canvas")
            .relative()
            .w(px(CANVAS_WIDTH))
            .h(px(CANVAS_HEIGHT))
            .rounded(px(6.0))
            .bg(gpui_kit::rgb(0x14141a))
            .border_1()
            .border_color(ui::track())
            .overflow_hidden();

        // 竖网格：四等分，标上片段内的秒数。
        for step in 0..=4 {
            let ratio = step as f32 / 4.0;
            let x = PADDING + ratio * (CANVAS_WIDTH - PADDING * 2.0);
            canvas = canvas.child(
                div()
                    .absolute()
                    .left(px(x - 0.5))
                    .top(px(PADDING))
                    .bottom(px(PADDING))
                    .w(px(1.0))
                    .bg(gpui_kit::rgba(0xffffff10)),
            );
            canvas = canvas.child(
                div()
                    .absolute()
                    .left(px((x - 14.0).max(1.0)))
                    .bottom(px(0.0))
                    .text_size(px(8.0))
                    .text_color(text_faint())
                    .child(format!("{:.1}s", duration.secs() * f64::from(ratio))),
            );
        }
        // 横网格：最小值 / 中点 / 最大值，标在右边。
        for (step, ratio) in [(0, 0.0f32), (1, 0.5), (2, 1.0)] {
            let value = property.min() + (property.max() - property.min()) * ratio;
            let y = y_of(value, property);
            canvas = canvas.child(
                div()
                    .absolute()
                    .top(px(y - 0.5))
                    .left(px(PADDING))
                    .right(px(PADDING))
                    .h(px(1.0))
                    .bg(gpui_kit::rgba(0xffffff0c)),
            );
            canvas = canvas.child(
                div()
                    .absolute()
                    .right(px(2.0))
                    .top(px((y - 6.0).max(0.0)))
                    .text_size(px(8.0))
                    .text_color(text_faint())
                    .child(property.format(value)),
            );
            let _ = step;
        }

        // 折线：按固定采样点数铺一串小横条。没点时就是一条水平线（静态值）。
        let static_value = item.keys.value(property, offset);
        let mut previous: Option<(f32, f32)> = None;
        for index in 0..SAMPLES {
            let t0 = duration.secs() * (index as f64 / SAMPLES as f64);
            let tick = Tick::from_secs(t0);
            let value = if points.is_empty() {
                static_value
            } else {
                item.keys.value(property, tick)
            };
            let (x, y) = (x_of(tick, duration), y_of(value, property));
            if let Some((px0, py0)) = previous {
                // 不旋转元素（旋转要算百分比、写起来也脆）：一列画一个竖条，
                // 从上一帧的 y 连到这一帧的 y，视觉上就是折线。
                let (left, width) = (px0.min(x), (x - px0).abs().max(1.0));
                let (top, height) = (py0.min(y), (y - py0).abs().max(1.5));
                canvas = canvas.child(
                    div()
                        .absolute()
                        .left(px(left))
                        .top(px(top))
                        .w(px(width))
                        .h(px(height))
                        .bg(ui::accent()),
                );
            }
            previous = Some((x, y));
        }

        // 关键帧菱形：点一下把播放头挪到那儿（下一帧起「当前段」就跟着变）。
        for (index, key) in points.iter().enumerate() {
            let (x, y) = (x_of(key.at, duration), y_of(key.value, property));
            let active = segment.is_some_and(|segment| segment == index);
            let key_at = key.at;
            canvas = canvas.child(
                div()
                    .id(("curve-key", index))
                    .absolute()
                    .left(px(x - 3.5))
                    .top(px(y - 3.5))
                    .w(px(7.0))
                    .h(px(7.0))
                    .rounded(px(2.0))
                    .bg(if active { ui::accent() } else { text_muted() })
                    .cursor(CursorStyle::PointingHand)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.seek_to_clip_offset(clip, key_at, cx);
                    })),
            );
        }

        // 贝塞尔手柄：只画当前段的那一对。
        if let Some(segment) = segment {
            let (from, to) = (points[segment], points[segment + 1]);
            let easing = from.easing_or(fallback);
            if easing.is_bezier() {
                let (x1, y1, x2, y2) = easing.controls();
                for (which, (cx_ratio, cy_ratio)) in [(1u8, (x1, y1)), (2, (x2, y2))] {
                    let x = x_of(from.at, duration)
                        + (x_of(to.at, duration) - x_of(from.at, duration)) * cx_ratio;
                    let y = y_of(from.value, property)
                        + (y_of(to.value, property) - y_of(from.value, property)) * cy_ratio;
                    canvas = canvas.child(
                        div()
                            .id(("curve-handle", which as usize))
                            .absolute()
                            .left(px(x - 4.0))
                            .top(px(y - 4.0))
                            .w(px(8.0))
                            .h(px(8.0))
                            .rounded(px(4.0))
                            .bg(bg())
                            .border_2()
                            .border_color(ui::accent())
                            .cursor(CursorStyle::PointingHand)
                            .on_drag(
                                CurveHandleDrag {
                                    clip,
                                    property,
                                    segment,
                                    which,
                                },
                                move |_drag, _offset, _window, cx| {
                                    cx.new(|_| super::timeline_tracks::TimelineGhost {
                                        name: SharedString::from("控制点"),
                                    })
                                },
                            ),
                    );
                }
            }
        }

        // 框选：在空白处按下拖动就是拉一个范围，松手删掉范围内的点。
        // 菱形自己带 `on_click`，点它不会变成框选。
        canvas = canvas.on_drag(
            CurveSelectDrag { clip, property },
            move |_drag, _offset, _window, cx| {
                cx.new(|_| super::timeline_tracks::TimelineGhost {
                    name: SharedString::from("框选"),
                })
            },
        );

        // 拖动中的框：只在真的在拖、且拖的是这个画布时画。
        if let Some(select) = self.curve_select
            && select.clip == clip
            && select.property == property
            && cx.has_active_drag()
        {
            let x0 = PADDING + select.from.0.min(select.to.0) * (CANVAS_WIDTH - PADDING * 2.0);
            let x1 = PADDING + select.from.0.max(select.to.0) * (CANVAS_WIDTH - PADDING * 2.0);
            let y_of_ratio = |ratio: f32| PADDING + (1.0 - ratio) * (CANVAS_HEIGHT - PADDING * 2.0);
            let y0 = y_of_ratio(select.from.1.max(select.to.1));
            let y1 = y_of_ratio(select.from.1.min(select.to.1));
            canvas = canvas.child(
                div()
                    .absolute()
                    .left(px(x0))
                    .top(px(y0))
                    .w(px((x1 - x0).max(1.0)))
                    .h(px((y1 - y0).max(1.0)))
                    .border_1()
                    .border_color(ui::accent())
                    .bg(gpui_kit::rgba(0x3f6fd833)),
            );
        }

        canvas
            .on_drag_move(cx.listener(
                move |this, event: &gpui_kit::DragMoveEvent<CurveHandleDrag>, _window, cx| {
                    let position = event.event.position;
                    if !event.bounds.contains(&position) {
                        return;
                    }
                    let (clip, property, segment, which) = {
                        let drag = event.drag(cx);
                        (drag.clip, drag.property, drag.segment, drag.which)
                    };
                    let x = f32::from(position.x) - f32::from(event.bounds.origin.x);
                    let y = f32::from(position.y) - f32::from(event.bounds.origin.y);
                    let control = to_curve(x, y);
                    this.curve_drop = Some(CurveDrop {
                        clip,
                        property,
                        segment,
                        which,
                        control,
                    });
                    cx.notify();
                },
            ))
            .on_drop(
                cx.listener(move |this, _drag: &CurveHandleDrag, _window, cx| {
                    this.drop_curve_handle(cx);
                }),
            )
            // 框选：另一套事件类型，与手柄互不干扰。
            .on_drag_move(cx.listener(
                move |this, event: &gpui_kit::DragMoveEvent<CurveSelectDrag>, _window, cx| {
                    let position = event.event.position;
                    if !event.bounds.contains(&position) {
                        return;
                    }
                    let (drag_clip, drag_property) = {
                        let drag = event.drag(cx);
                        (drag.clip, drag.property)
                    };
                    let x = f32::from(position.x) - f32::from(event.bounds.origin.x);
                    let y = f32::from(position.y) - f32::from(event.bounds.origin.y);
                    let to = to_curve(x, y);
                    this.curve_select = Some(match this.curve_select {
                        Some(select)
                            if select.clip == drag_clip && select.property == drag_property =>
                        {
                            CurveSelect { to, ..select }
                        }
                        _ => CurveSelect {
                            clip: drag_clip,
                            property: drag_property,
                            from: to,
                            to,
                        },
                    });
                    cx.notify();
                },
            ))
            .on_drop(
                cx.listener(move |this, _drag: &CurveSelectDrag, _window, cx| {
                    this.drop_curve_selection(cx);
                }),
            )
            .into_any_element()
    }

    /// 松手：把拖出来的控制点写回那个关键帧。
    pub(super) fn drop_curve_handle(&mut self, cx: &mut Context<Self>) {
        let Some(drop) = self.curve_drop.take() else {
            return;
        };
        self.edit_keys(drop.clip, cx, |sequence| {
            let item = sequence.clip_mut(drop.clip)?;
            let curve = item.keys.ensure(drop.property);
            let current = curve
                .keys
                .get(drop.segment)
                .map(|key| key.easing_or(curve.interpolation))
                .unwrap_or_default();
            let (mut x1, mut y1, mut x2, mut y2) = current.controls();
            if drop.which == 1 {
                x1 = drop.control.0;
                y1 = drop.control.1;
            } else {
                x2 = drop.control.0;
                y2 = drop.control.1;
            }
            let key = curve.keys.get_mut(drop.segment)?;
            key.easing = Some(Interpolation::with_controls(x1, y1, x2, y2));
            Some(format!(
                "{}：控制点 ({:.2}, {:.2}) / ({:.2}, {:.2})",
                drop.property.label(),
                x1,
                y1,
                x2,
                y2
            ))
        });
        cx.notify();
    }

    /// 框选松手：删掉范围里的关键帧。一个都不剩时曲线退回「没有点」，
    /// 属性回到静态默认值——这也是框选的常见用法（整条清掉重打）。
    pub(super) fn drop_curve_selection(&mut self, cx: &mut Context<Self>) {
        let Some(select) = self.curve_select.take() else {
            return;
        };
        // 只点了一下没有拖出范围：当没发生，免得误删。
        if (select.from.0 - select.to.0).abs() < 0.01 && (select.from.1 - select.to.1).abs() < 0.01
        {
            cx.notify();
            return;
        }
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let Some(item) = project.sequence().clip(select.clip) else {
            return;
        };
        let duration = item.duration();
        let Some(curve) = item.keys.curve(select.property) else {
            return;
        };
        let (min, max) = (select.property.min(), select.property.max());
        let span = (max - min).max(0.0001);
        let doomed: Vec<Tick> = curve
            .keys
            .iter()
            .filter(|key| {
                let at = if duration.secs() > 0.0 {
                    (key.at.secs() / duration.secs()) as f32
                } else {
                    0.0
                };
                select.contains(at, (key.value - min) / span)
            })
            .map(|key| key.at)
            .collect();
        if doomed.is_empty() {
            self.flash("框里没有关键帧");
            cx.notify();
            return;
        }
        let count = doomed.len();
        self.edit_keys(select.clip, cx, |sequence| {
            let item = sequence.clip_mut(select.clip)?;
            let curve = item.keys.ensure(select.property);
            for at in &doomed {
                curve.remove_near(*at, Tick::ZERO);
            }
            Some(format!(
                "{}：框选删掉 {count} 个关键帧",
                select.property.label()
            ))
        });
        cx.notify();
    }

    /// 复制这条曲线的所有点（框选之外的第二条路：整条曲线搬去别的属性 / 片段）。
    pub(super) fn copy_curve(&mut self, clip: Id, property: Property, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let Some(curve) = project
            .sequence()
            .clip(clip)
            .and_then(|item| item.keys.curve(property))
        else {
            self.flash("这条曲线还没有关键帧");
            cx.notify();
            return;
        };
        if curve.keys.is_empty() {
            self.flash("这条曲线还没有关键帧");
            cx.notify();
            return;
        }
        self.key_clipboard = Some(curve.keys.clone());
        self.flash(format!(
            "{}：复制了 {} 个关键帧",
            property.label(),
            curve.keys.len()
        ));
        cx.notify();
    }

    /// 把剪贴板里的点粘到当前属性（覆盖原有曲线）。
    pub(super) fn paste_curve(&mut self, clip: Id, property: Property, cx: &mut Context<Self>) {
        let Some(keys) = self.key_clipboard.clone() else {
            self.flash("剪贴板是空的：先「复制」一条曲线");
            cx.notify();
            return;
        };
        let count = keys.len();
        self.edit_keys(clip, cx, |sequence| {
            let item = sequence.clip_mut(clip)?;
            let curve = item.keys.ensure(property);
            curve.keys = keys;
            // 粘过来的点可能超出这条属性的取值范围（不透明度粘到缩放上）。
            for key in &mut curve.keys {
                key.value = key.value.clamp(property.min(), property.max());
            }
            Some(format!("{}：粘贴 {count} 个关键帧", property.label()))
        });
        cx.notify();
    }

    /// 「复制 / 粘贴」两颗小按钮。
    pub(super) fn render_curve_clipboard(
        &self,
        clip: Id,
        property: Property,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let has_keys = self
            .project
            .as_ref()
            .and_then(|project| project.sequence().clip(clip))
            .is_some_and(|item| {
                item.keys
                    .curve(property)
                    .is_some_and(|curve| !curve.keys.is_empty())
            });
        let has_clipboard = self.key_clipboard.is_some();
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(settings_button(
                "curve-copy",
                "复制曲线",
                has_keys,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.copy_curve(clip, property, cx);
                }),
            ))
            .child(settings_button(
                "curve-paste",
                "粘贴到这里",
                has_clipboard,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.paste_curve(clip, property, cx);
                }),
            ))
            .into_any_element()
    }

    /// 点曲线上的菱形：把播放头挪到片段内那个时刻。
    fn seek_to_clip_offset(&mut self, clip: Id, at: Tick, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let Some(item) = project.sequence().clip(clip) else {
            return;
        };
        let duration = project.sequence().duration();
        let target = (item.start + at).clamp(Tick::ZERO, duration);
        if let Some(project) = self.project.as_mut() {
            project.ui.playhead = target;
        }
        // 与「跳到时间码」同一条路径：挪完重新出一帧，画面跟着走。
        self.preview_at_playhead(cx);
        cx.notify();
    }
}

/// 两套缓动是不是同一个档位（预设按钮的选中态）。
fn same_easing(a: Interpolation, b: Interpolation) -> bool {
    match (a, b) {
        (Interpolation::Linear, Interpolation::Linear) => true,
        (Interpolation::EaseInOut, Interpolation::EaseInOut) => true,
        (
            Interpolation::Bezier {
                x1: ax1,
                y1: ay1,
                x2: ax2,
                y2: ay2,
            },
            Interpolation::Bezier {
                x1: bx1,
                y1: by1,
                x2: bx2,
                y2: by2,
            },
        ) => {
            (ax1 - bx1).abs() < 1e-3
                && (ay1 - by1).abs() < 1e-3
                && (ax2 - bx2).abs() < 1e-3
                && (ay2 - by2).abs() < 1e-3
        }
        _ => false,
    }
}

const CURVE_TAB_IDS: [&str; 4] = [
    "curve-tab-scale",
    "curve-tab-x",
    "curve-tab-y",
    "curve-tab-opacity",
];

const CURVE_PRESET_IDS: [&str; 5] = [
    "curve-preset-linear",
    "curve-preset-ease-in-out",
    "curve-preset-ease-in",
    "curve-preset-ease-out",
    "curve-preset-strong",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 画布坐标与取值能往返() {
        let duration = Tick::from_secs(4.0);
        for (seconds, value) in [(0.0, 0.1f32), (1.0, 1.0), (2.5, 2.0), (4.0, 4.0)] {
            let x = x_of(Tick::from_secs(seconds), duration);
            let y = y_of(value, Property::Scale);
            let (back_x, back_y) = to_curve(x, y);
            assert!(
                (back_x - seconds as f32 / 4.0).abs() < 0.02,
                "{seconds}s -> {back_x}"
            );
            let expected =
                (value - Property::Scale.min()) / (Property::Scale.max() - Property::Scale.min());
            assert!((back_y - expected).abs() < 0.03, "{value} -> {back_y}");
        }
    }

    #[test]
    fn 拖出画布时夹在边界() {
        assert_eq!(to_curve(-50.0, -50.0), (0.0, 1.0));
        assert_eq!(
            to_curve(CANVAS_WIDTH + 50.0, CANVAS_HEIGHT + 50.0),
            (1.0, 0.0)
        );
    }

    #[test]
    fn 取值越大画得越高() {
        // 纵轴向下，所以大值的 y 更小。
        assert!(
            y_of(Property::Scale.max(), Property::Scale)
                < y_of(Property::Scale.min(), Property::Scale)
        );
    }

    #[test]
    fn 预设按钮的选中态只认同一个档位() {
        let ease = Interpolation::Bezier {
            x1: 0.42,
            y1: 0.0,
            x2: 0.58,
            y2: 1.0,
        };
        assert!(same_easing(ease, ease));
        assert!(!same_easing(ease, Interpolation::Linear));
        assert!(same_easing(Interpolation::Linear, Interpolation::Linear));
        // 粘手拖出来的微小差别也算同一个，否则按钮永远不亮。
        let near = Interpolation::with_controls(0.4201, 0.0, 0.58, 1.0);
        assert!(same_easing(ease, near));
    }
}
