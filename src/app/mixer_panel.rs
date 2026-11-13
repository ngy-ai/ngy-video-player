//! 混音台：每条音频轨的推子、声像、独奏 / 静音与音量自动化。
//!
//! 面板是模态卡片（一排一条轨），推子与声像条都是**按住拖动**的：滑块下面垫一层
//! `canvas` 记下实际像素范围，拖动时按指针位置换算，不按「拖了多远」累加——累加的
//! 话指针一快就会掉队，松手时的值也和指针位置对不上。
//!
//! 推子走 **dB 刻度**：线性刻度下 -40 dB 那一段被挤在最左边几像素里，用户根本拖不准；
//! dB 刻度下每一像素差不多 0.35 dB，看着和 DAW 一致。最左 1% 是静音档（增益 0）。
//!
//! 三层音量（片段 → 自动化 → 推子）与声像的算法在 `timeline::mixer`，这里只做编辑；
//! 预览侧同一刻用同一个函数算增益，所以在这里推一下就能听见。

use super::*;
use crate::timeline::mixer::{self, GainPoint};
use crate::timeline::{Id, TrackKind};
use std::rc::Rc;

pub(super) mod automation;

/// 面板本身没有状态：所有值都在工程里，拖动状态在 `PlayerApp`。
#[derive(Default)]
pub(super) struct MixerPanel;

/// 一条轨上可拖的两个条。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MixerSlot {
    Fader,
    Pan,
}

/// 正在拖的那一条。
pub(super) struct MixerDrag {
    pub(super) track: Id,
    pub(super) slot: MixerSlot,
    /// 条的像素范围，由 `canvas` 在绘制时写进来。
    pub(super) bounds: Rc<Cell<Bounds<Pixels>>>,
}

/// 推子的 dB 量程：-60 dB 到 `mixer::MAX_GAIN` 对应的 +6.02 dB。
///
/// 顶上不写死 6.0：那样推到最高也只有 1.995，永远差 0.04 才到上限，看着像推不上去。
const FADER_MIN_DB: f32 = -60.0;
const FADER_MAX_DB: f32 = 6.0206;
/// 最左这一小段算静音：留一点死区，否则「拖到最左」总是停在 -60 dB。
const FADER_SILENT_BELOW: f32 = 0.01;

impl PlayerApp {
    pub(super) fn open_mixer_panel(&mut self, cx: &mut Context<Self>) {
        if self.project.is_none() {
            self.flash("先打开工程，再调混音台");
            cx.notify();
            return;
        }
        // 模态互斥：同时开两个会抢同一份按键。
        self.close_export_dialog(cx);
        self.close_subtitle_dialog(cx);
        self.close_sequence_dialog(cx);
        self.close_sequence_manager_dialog(cx);
        self.close_metadata_dialog(cx);
        self.close_proxy_dialog(cx);
        self.close_scope_dialog(cx);
        self.close_scenes_dialog(cx);
        self.close_multicam_dialog(cx);
        self.close_concat_dialog(cx);
        self.close_remux_dialog(cx);
        self.close_delete_track_confirm(cx);
        self.project_confirm = None;
        self.show_settings = false;
        self.timeline_menu = None;
        self.menu = None;
        self.menu_bar = None;
        self.mixer_panel = Some(MixerPanel);
        cx.notify();
    }

    pub(super) fn close_mixer_panel(&mut self, cx: &mut Context<Self>) {
        self.mixer_dragging = None;
        self.mixer_auto_drag = None;
        if self.mixer_panel.take().is_some() {
            cx.notify();
        }
    }

    /// 拖动中：按指针位置设值。
    pub(super) fn drag_mixer(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let Some(drag) = self.mixer_dragging.as_ref() else {
            return;
        };
        let bounds = drag.bounds.get();
        let (track, slot) = (drag.track, drag.slot);
        let width = f32::from(bounds.size.width);
        if width <= 1.0 {
            return;
        }
        let ratio = ((f32::from(x) - f32::from(bounds.origin.x)) / width).clamp(0.0, 1.0);
        match slot {
            MixerSlot::Fader => self.set_track_gain(track, gain_from_ratio(ratio), cx),
            MixerSlot::Pan => self.set_track_pan(track, pan_from_ratio(ratio), cx),
        }
    }

    /// 改声像。值没变就什么都不做（拖动时每像素都会进来一次）。
    pub(super) fn set_track_pan(&mut self, track: Id, pan: f32, cx: &mut Context<Self>) {
        let Some((message, changed)) = self
            .project
            .as_mut()
            .and_then(|project| project.sequence_mut().track_mut(track))
            .map(|target| {
                let pan = mixer::normalized_pan(pan);
                let changed = (mixer::normalized_pan(target.pan) - pan).abs() > 1e-3;
                target.pan = pan;
                (
                    format!("轨道「{}」声像 {}", target.name, pan_label(pan)),
                    changed,
                )
            })
        else {
            return;
        };
        if !changed {
            return;
        }
        self.remember_history(message.clone());
        self.flash(message);
        // 声像只影响预览的声音，画面不用重取；刷新音频实例即可。
        self.preview_at_playhead(cx);
        cx.notify();
    }

    /// 在播放头记一个自动化点（取当前推子的值）。
    pub(super) fn add_automation_point(&mut self, track: Id, cx: &mut Context<Self>) {
        let Some(playhead) = self.project.as_ref().map(|project| project.ui.playhead) else {
            return;
        };
        let Some((message, changed)) = self
            .project
            .as_mut()
            .and_then(|project| project.sequence_mut().track_mut(track))
            .map(|target| {
                let gain = mixer::clip_gain(target.gain);
                let point = GainPoint::new(playhead, gain);
                // 同一帧上只留一个点：拖到同一位置再记一次应当覆盖，不是叠两个。
                let before = target.automation.clone();
                target
                    .automation
                    .retain(|existing| (existing.at - playhead).micros().abs() > 1_000);
                target.automation.push(point);
                target.automation.sort_by_key(|point| point.at);
                (
                    format!(
                        "轨道「{}」在播放头记一个自动化点 {}",
                        target.name,
                        mixer::db_label(gain)
                    ),
                    target.automation != before,
                )
            })
        else {
            return;
        };
        if !changed {
            return;
        }
        self.remember_history(message.clone());
        self.flash(message);
        self.preview_at_playhead(cx);
        cx.notify();
    }

    /// 删掉离播放头最近的自动化点（一帧以内才算「在播放头上」）。
    pub(super) fn remove_automation_point(&mut self, track: Id, cx: &mut Context<Self>) {
        let Some(playhead) = self.project.as_ref().map(|project| project.ui.playhead) else {
            return;
        };
        let mut nearest = None;
        if let Some(target) = self
            .project
            .as_mut()
            .and_then(|project| project.sequence_mut().track_mut(track))
            && !target.automation.is_empty()
        {
            // 找最近的那个点（不要求正好压在同一帧上：用户看到的是那条线的拐点）。
            let mut best = 0;
            for (index, point) in target.automation.iter().enumerate() {
                if (point.at - playhead).micros().abs()
                    < (target.automation[best].at - playhead).micros().abs()
                {
                    best = index;
                }
            }
            let point = target.automation.remove(best);
            nearest = Some((
                format!(
                    "轨道「{}」删掉自动化点 {}",
                    target.name,
                    mixer::db_label(point.gain)
                ),
                point,
            ));
        }
        match nearest {
            Some((message, _)) => {
                self.remember_history(message.clone());
                self.flash(message);
                self.preview_at_playhead(cx);
            }
            None => self.flash("这条轨还没有自动化点"),
        }
        cx.notify();
    }

    /// 清空自动化：回到「只有推子」的状态。
    pub(super) fn clear_automation(&mut self, track: Id, cx: &mut Context<Self>) {
        let Some((message, changed)) = self
            .project
            .as_mut()
            .and_then(|project| project.sequence_mut().track_mut(track))
            .map(|target| {
                let changed = !target.automation.is_empty();
                target.automation.clear();
                (format!("轨道「{}」清空音量自动化", target.name), changed)
            })
        else {
            return;
        };
        if !changed {
            self.flash("这条轨没有自动化点");
            cx.notify();
            return;
        }
        self.remember_history(message.clone());
        self.flash(message);
        self.preview_at_playhead(cx);
        cx.notify();
    }
}

impl PlayerApp {
    /// 混音台面板。
    pub(super) fn render_mixer_panel(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.mixer_panel.as_ref()?;
        let project = self.project.as_ref()?;
        let sequence = project.sequence();
        let playhead = project.ui.playhead;
        let duration = sequence.duration();
        let audio: Vec<_> = sequence
            .tracks
            .iter()
            .filter(|track| track.kind == TrackKind::Audio)
            .collect();
        if audio.is_empty() {
            return Some(modal_card_width(
                "mixer-panel-card",
                "混音台",
                460.0,
                div()
                    .text_size(px(12.0))
                    .text_color(text_muted())
                    .child("这个序列还没有音频轨：先在时间线上加一条音频轨再回来。")
                    .into_any_element(),
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.close_mixer_panel(cx);
                }),
            ));
        }

        let mut rows = div().flex().flex_col().gap(px(10.0));
        for track in audio {
            rows = rows.child(self.render_mixer_row(track, playhead, duration, cx));
        }

        let card = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div().flex().items_center().gap(px(8.0)).child(
                    div()
                        .text_size(px(11.0))
                        .text_color(text_muted())
                        .child(format!(
                            "{} 条音频轨 · 推子与声像条按住拖动",
                            sequence
                                .tracks
                                .iter()
                                .filter(|track| track.kind == TrackKind::Audio)
                                .count()
                        )),
                ),
            )
            .child(rows)
            .child(div().text_size(px(10.0)).text_color(text_faint()).child(
                "下面的小画布是音量自动化：横轴是序列时间，纵轴和推子同一套 dB 刻度；\
                         按住圆点拖到任意时刻与音量，松手才算一步撤销。\
                         「复制曲线 / 粘贴」可以把一条轨的点整条搬到另一条轨。\
                         导出时按 `volume` 表达式逐帧渲染，预览跟着播放头走同一条线。",
            ));

        // 一行要塞下名字 / M / S / 推子 / dB / 声像 / 左右 / 三个自动化按钮，
        // 460 的常规宽度不够；窗口窄时靠 `max_w` 收回来。
        Some(modal_card_width(
            "mixer-panel-card",
            "混音台",
            860.0,
            card.into_any_element(),
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.close_mixer_panel(cx);
            }),
        ))
    }

    /// 一条轨的一行。
    fn render_mixer_row(
        &self,
        track: &crate::timeline::Track,
        playhead: Tick,
        duration: Tick,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = track.id;
        let gain = track.normalized_gain();
        let pan = mixer::normalized_pan(track.pan);
        let auto_at = mixer::automation_at(&track.automation, playhead);
        let muted = track.muted;
        let solo = track.solo;
        let locked = track.locked;
        let points = track.automation.len();

        let fader = self.slider(
            id,
            MixerSlot::Fader,
            ratio_from_gain(gain),
            ui::accent(),
            false,
            cx,
        );
        let pan_bar = self.slider(
            id,
            MixerSlot::Pan,
            ratio_from_pan(pan),
            ui::accent(),
            true,
            cx,
        );

        let controls = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .h(px(30.0))
            .child(
                div()
                    .w(px(116.0))
                    .flex_none()
                    .truncate()
                    .text_size(px(11.0))
                    .text_color(if muted { text_faint() } else { text() })
                    .child(track.name.clone()),
            )
            .child(chip(
                "mixer-mute",
                "M",
                muted,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.toggle_track_muted(id, cx);
                }),
            ))
            .child(chip(
                "mixer-solo",
                "S",
                solo,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.toggle_track_solo(id, cx);
                }),
            ))
            .child(fader)
            .child(
                div()
                    .w(px(58.0))
                    .flex_none()
                    .text_size(px(10.0))
                    .text_color(if locked { text_faint() } else { text_muted() })
                    .child(mixer::db_label(gain)),
            )
            .child(pan_bar)
            .child(
                div()
                    .w(px(54.0))
                    .flex_none()
                    .text_size(px(10.0))
                    .text_color(text_muted())
                    .child(pan_label(pan)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .child(chip(
                        "mixer-auto-add",
                        "＋点",
                        false,
                        cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.add_automation_point(id, cx);
                        }),
                    ))
                    .child(chip(
                        "mixer-auto-remove",
                        "－点",
                        false,
                        cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.remove_automation_point(id, cx);
                        }),
                    ))
                    .child(chip(
                        "mixer-auto-clear",
                        "清空",
                        false,
                        cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.clear_automation(id, cx);
                        }),
                    ))
                    .child(
                        div()
                            .w(px(52.0))
                            .text_size(px(10.0))
                            .text_color(text_faint())
                            .child(match auto_at {
                                Some(value) if points > 1 => {
                                    format!("{}·{points}点", mixer::db_label(value))
                                }
                                Some(value) => mixer::db_label(value),
                                None => format!("{points} 点"),
                            }),
                    ),
            );

        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(controls)
            .child(self.automation_lane(track, duration, cx))
            .into_any_element()
    }

    /// 一根可拖的条。`centered` 为真时填充从中间往两边长（声像）。
    fn slider(
        &self,
        track: Id,
        slot: MixerSlot,
        ratio: f32,
        color: Hsla,
        centered: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let bounds: Rc<Cell<Bounds<Pixels>>> = Rc::new(Cell::new(Bounds::default()));
        let captured = bounds.clone();
        let width = if centered { 108.0 } else { 168.0 };
        let fill = if centered {
            // 声像：从中间到当前位置。
            let (left, width_ratio) = if ratio >= 0.5 {
                (0.5, ratio - 0.5)
            } else {
                (ratio, 0.5 - ratio)
            };
            div()
                .absolute()
                .left(gpui_kit::relative(left))
                .top_0()
                .bottom_0()
                .w(gpui_kit::relative(width_ratio))
                .rounded(px(2.0))
                .bg(color)
                .into_any_element()
        } else {
            div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(gpui_kit::relative(ratio))
                .rounded(px(2.0))
                .bg(color)
                .into_any_element()
        };
        let pointer: Rc<Cell<Bounds<Pixels>>> = bounds.clone();
        let element_id = format!(
            "mixer-slider-{}-{}",
            track.raw(),
            match slot {
                MixerSlot::Fader => "fader",
                MixerSlot::Pan => "pan",
            }
        );
        div()
            .id(SharedString::from(element_id))
            .relative()
            .flex_none()
            .w(px(width))
            .h(px(18.0))
            .flex()
            .items_center()
            .cursor(CursorStyle::PointingHand)
            .child(
                canvas(
                    move |bounds, _window, _cx| captured.set(bounds),
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
                    .child(fill),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.mixer_dragging = Some(MixerDrag {
                        track,
                        slot,
                        bounds: pointer.clone(),
                    });
                    this.drag_mixer(event.position.x, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _window, cx| {
                    this.mixer_dragging = None;
                    cx.notify();
                }),
            )
            .into_any_element()
    }
}

/// 推子：条上的比例 → 线性增益（dB 刻度）。（`_tag` 只为日后加日志时不用改调用处。）
fn gain_from_ratio(ratio: f32) -> f32 {
    let ratio = ratio.clamp(0.0, 1.0);
    if ratio <= FADER_SILENT_BELOW {
        return 0.0;
    }
    let db = FADER_MIN_DB + ratio * (FADER_MAX_DB - FADER_MIN_DB);
    (10f32.powf(db / 20.0)).clamp(0.0, mixer::MAX_GAIN)
}

/// 线性增益 → 条上的比例。
fn ratio_from_gain(gain: f32) -> f32 {
    let gain = mixer::clip_gain(gain);
    if gain <= 1e-4 {
        return 0.0;
    }
    let db = 20.0 * gain.log10();
    ((db - FADER_MIN_DB) / (FADER_MAX_DB - FADER_MIN_DB)).clamp(0.0, 1.0)
}

/// 声像条：比例 → -1..1。
fn pan_from_ratio(ratio: f32) -> f32 {
    (ratio.clamp(0.0, 1.0) - 0.5) * 2.0
}

/// 声像 → 比例。
fn ratio_from_pan(pan: f32) -> f32 {
    (mixer::normalized_pan(pan) * 0.5 + 0.5).clamp(0.0, 1.0)
}

/// 声像文案：`L50` / `R30` / `居中`。
fn pan_label(pan: f32) -> String {
    let pan = mixer::normalized_pan(pan);
    if pan.abs() < 0.01 {
        return "居中".to_string();
    }
    format!(
        "{}{:.0}",
        if pan < 0.0 { "L" } else { "R" },
        pan.abs() * 100.0
    )
}

#[cfg(test)]
mod tests;
