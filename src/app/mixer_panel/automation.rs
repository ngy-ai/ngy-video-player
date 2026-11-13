//! 音量自动化的可拖画布与曲线复制 / 粘贴。
//!
//! 「在播放头打 / 删 / 清空」三个按钮仍在 `mixer_panel.rs`；这里补上「看得见的点」：
//! 每条音频轨一行小画布，横轴是序列时间、纵轴用与推子相同的 dB 刻度，点能按住拖到
//! 任意时刻 / 增益。拖动中只记落点、画布用预览值重画，松手才写回工程并压一步历史。
//!
//! 复制 / 粘贴是跨轨的：在 A 轨复制整条曲线，到 B 轨粘贴（时间原样覆盖、不按播放头偏移，
//! 与关键帧曲线编辑器的口径一致）。

use std::cell::Cell;
use std::rc::Rc;

use super::*;
use crate::timeline::mixer::{self, GainPoint};
use crate::timeline::{Id, Tick};

/// 画布高度。点直径 8px，留够上下边界让首尾点也能点中。
const LANE_HEIGHT: f32 = 34.0;
const DOT: f32 = 8.0;

/// 正在拖的自动化点。`index` 是按下时的下标（拖动中数组不变，画布用预览值重画）。
pub(in crate::app) struct MixerAutoDrag {
    pub(in crate::app) track: Id,
    pub(in crate::app) index: usize,
    pub(in crate::app) at: Tick,
    pub(in crate::app) gain: f32,
    /// 拖之前的点，用来判断松手时到底有没有变。
    pub(in crate::app) before: GainPoint,
    pub(in crate::app) bounds: Rc<Cell<Bounds<Pixels>>>,
}

/// 时间 → 画布横向比例（0..1）。
pub(super) fn auto_x(at: Tick, duration: Tick) -> f32 {
    let span = duration.micros();
    if span <= 0 {
        return 0.0;
    }
    (at.micros() as f32 / span as f32).clamp(0.0, 1.0)
}

/// 横向比例 → 时间。
pub(super) fn auto_at(ratio: f32, duration: Tick) -> Tick {
    let micros = (f64::from(ratio.clamp(0.0, 1.0)) * duration.micros().max(0) as f64).round();
    Tick::from_micros((micros as i64).max(0))
}

/// 增益 → 画布纵向比例（0 在顶 = +6 dB，1 在底 = -60 dB / 静音）。
pub(super) fn auto_y(gain: f32) -> f32 {
    1.0 - ratio_from_gain(gain)
}

/// 纵向比例 → 增益。
pub(super) fn auto_gain(y: f32) -> f32 {
    gain_from_ratio(1.0 - y.clamp(0.0, 1.0))
}

/// 写回前收口：钳制坏值、按时间排序、同一毫秒只留一个点（留后来的那个）。
pub(super) fn normalize_automation(points: &mut Vec<GainPoint>) {
    for point in points.iter_mut() {
        point.gain = mixer::clip_gain(point.gain);
        point.at = point.at.max(Tick::ZERO);
    }
    points.sort_by_key(|point| point.at);
    let mut index = 1;
    while index < points.len() {
        if (points[index].at - points[index - 1].at).micros().abs() <= 1_000 {
            points.remove(index - 1);
        } else {
            index += 1;
        }
    }
}

impl PlayerApp {
    /// 一条轨的自动化画布 + 复制 / 粘贴按钮。放在推子那一行下面，不挤原有布局。
    pub(super) fn automation_lane(
        &self,
        track: &crate::timeline::Track,
        duration: Tick,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = track.id;
        let bounds: Rc<Cell<Bounds<Pixels>>> = Rc::new(Cell::new(Bounds::default()));
        let captured = bounds.clone();
        let pointer = bounds.clone();
        let mut shown = track.automation.clone();
        let dragging = self
            .mixer_auto_drag
            .as_ref()
            .filter(|drag| drag.track == id)
            .map(|drag| (drag.index, drag.at, drag.gain));
        if let Some((index, at, gain)) = dragging
            && let Some(slot) = shown.get_mut(index)
        {
            *slot = GainPoint::new(at, gain);
        }

        let mut lane = div()
            .relative()
            .flex_1()
            .min_w(px(0.0))
            .h(px(LANE_HEIGHT))
            .rounded(px(4.0))
            .bg(gpui_kit::rgba(0x0000_0033))
            .border_1()
            .border_color(ui::track())
            .child(
                canvas(
                    move |rect, _window, _cx| captured.set(rect),
                    |_rect, (), _window, _cx| {},
                )
                .absolute()
                .inset_0(),
            )
            // 0 dB 参考线：拖点时能看出是加还是减。
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(gpui_kit::relative(auto_y(1.0)))
                    .h(px(1.0))
                    .bg(gpui_kit::rgba(0xffff_ff22)),
            );

        for (index, point) in shown.iter().copied().enumerate() {
            let x = auto_x(point.at, duration);
            let y = auto_y(point.gain);
            let active = dragging.is_some_and(|(dragged, _, _)| dragged == index);
            let ink: Hsla = if active {
                ui::accent()
            } else {
                gpui_kit::rgb(0xffd166).into()
            };
            let dot_bounds = pointer.clone();
            lane = lane.child(
                div()
                    .id(("mixer-auto-point", index))
                    .absolute()
                    .left(gpui_kit::relative(x))
                    .top(gpui_kit::relative(y))
                    .w(px(DOT))
                    .h(px(DOT))
                    .ml(px(-DOT * 0.5))
                    .mt(px(-DOT * 0.5))
                    .rounded_full()
                    .border_1()
                    .border_color(gpui_kit::rgb(0x101014))
                    .bg(ink)
                    .cursor(CursorStyle::PointingHand)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event: &MouseDownEvent, _window, cx| {
                            cx.stop_propagation();
                            this.mixer_auto_drag = Some(MixerAutoDrag {
                                track: id,
                                index,
                                at: point.at,
                                gain: point.gain,
                                before: point,
                                bounds: dot_bounds.clone(),
                            });
                            cx.notify();
                        }),
                    ),
            );
        }

        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(lane)
            .child(settings_button(
                format!("mixer-auto-copy-{}", id.raw()),
                "复制曲线",
                true,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.copy_automation(id, cx);
                }),
            ))
            .child(settings_button(
                format!("mixer-auto-paste-{}", id.raw()),
                "粘贴",
                self.automation_clipboard.is_some(),
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.paste_automation(id, cx);
                }),
            ))
            .into_any_element()
    }

    /// 拖动中：按指针在画布里的位置换算时间与增益，只改落点不碰工程。
    pub(in crate::app) fn drag_automation_point(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.mixer_auto_drag.as_ref() else {
            return;
        };
        let bounds = drag.bounds.get();
        let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let Some(duration) = self
            .project
            .as_ref()
            .map(|project| project.sequence().duration())
        else {
            return;
        };
        if width <= 1.0 || height <= 1.0 || duration.micros() <= 0 {
            return;
        }
        let x = ((f32::from(position.x) - f32::from(bounds.origin.x)) / width).clamp(0.0, 1.0);
        let y = ((f32::from(position.y) - f32::from(bounds.origin.y)) / height).clamp(0.0, 1.0);
        let (at, gain) = (auto_at(x, duration), auto_gain(y));
        if let Some(drag) = self.mixer_auto_drag.as_mut() {
            drag.at = at;
            drag.gain = gain;
        }
        cx.notify();
    }

    /// 松手：写回工程并压一步历史。没变就不留空操作。
    pub(in crate::app) fn drop_automation_point(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.mixer_auto_drag.take() else {
            return;
        };
        let point = GainPoint::new(drag.at, drag.gain);
        if point == drag.before {
            cx.notify();
            return;
        }
        let Some((message, changed)) = self
            .project
            .as_mut()
            .and_then(|project| project.sequence_mut().track_mut(drag.track))
            .map(|target| {
                let Some(slot) = target.automation.get_mut(drag.index) else {
                    return (String::new(), false);
                };
                *slot = point;
                normalize_automation(&mut target.automation);
                (
                    format!(
                        "轨道「{}」移动自动化点到 {:.2}s · {}",
                        target.name,
                        point.at.secs(),
                        mixer::db_label(point.gain)
                    ),
                    true,
                )
            })
        else {
            cx.notify();
            return;
        };
        if !changed {
            cx.notify();
            return;
        }
        self.remember_history(message.clone());
        self.flash(message);
        self.preview_at_playhead(cx);
        cx.notify();
    }

    /// 复制整条曲线到会话剪贴板（不随工程存盘）。
    pub(super) fn copy_automation(&mut self, track: Id, cx: &mut Context<Self>) {
        let points = self
            .project
            .as_ref()
            .and_then(|project| project.sequence().track(track))
            .map(|target| target.automation.clone());
        let Some(points) = points else {
            return;
        };
        if points.is_empty() {
            self.flash("这条轨还没有自动化点");
            cx.notify();
            return;
        }
        let count = points.len();
        self.automation_clipboard = Some(points);
        self.flash(format!("已复制 {count} 个自动化点，到别的轨点「粘贴」"));
        cx.notify();
    }

    /// 粘贴：整条覆盖目标轨，时间原样、增益钳到量程内。
    pub(super) fn paste_automation(&mut self, track: Id, cx: &mut Context<Self>) {
        let Some(mut points) = self.automation_clipboard.clone() else {
            self.flash("先复制一条轨的自动化曲线");
            cx.notify();
            return;
        };
        normalize_automation(&mut points);
        let Some((message, changed)) = self
            .project
            .as_mut()
            .and_then(|project| project.sequence_mut().track_mut(track))
            .map(|target| {
                let changed = target.automation != points;
                target.automation = points.clone();
                (
                    format!("轨道「{}」粘贴 {} 个自动化点", target.name, points.len()),
                    changed,
                )
            })
        else {
            return;
        };
        if !changed {
            self.flash("这条轨的曲线和剪贴板里的一样");
            cx.notify();
            return;
        }
        self.remember_history(message.clone());
        self.flash(message);
        self.preview_at_playhead(cx);
        cx.notify();
    }
}
