//! 时间线的轨道行与片段：绘制、拖动换轨、静音与锁定开关。

use super::*;
use crate::timeline::{EditOp, Id, Rational, apply as apply_edit};
use gpui_kit::DragMoveEvent;

/// 拖动中的片段。GPUI 的拖放按值的类型分发，专用类型不会和「从资源管理器拖文件进来」混在一起。
struct TimelineDrag {
    clip: Id,
    /// 抓取点在片段内部的横向像素偏移。落点按片段左边缘算，不按鼠标位置算，
    /// 否则抓住长片段中间拖动时，片段会整个跳到鼠标下面。
    grab: Rc<Cell<f32>>,
}

/// 跟着鼠标走的拖动预览：一颗只显示名字的胶囊。
struct TimelineGhost {
    name: SharedString,
}

impl Render for TimelineGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(180.0))
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(4.0))
            .bg(gpui_kit::rgba(0x232330f2))
            .border_1()
            .border_color(ui::accent())
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_size(px(11.0))
            .text_color(text())
            .child(self.name.clone())
    }
}

/// 拖动片段时算出来的落点。渲染预览与松手后执行都用它。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TimelineDrop {
    clip: Id,
    to_track: Id,
    start: Tick,
}

impl PlayerApp {
    /// 一条轨道上的片段。位置按视口换算，滚动时只画看得见的那几段。
    ///
    /// 片段可以拖到别的同类型轨道上；落点由 `on_drag_move` 算好放进
    /// `self.timeline_drop`，松手时 `on_drop` 才真正改序列。
    pub(super) fn render_timeline_lane(
        &self,
        project: &Project,
        track: &crate::timeline::Track,
        top: f32,
        lane_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view = &self.timeline;
        let visible_end = view.scroll() + view.visible_seconds();
        let track_id = track.id;
        let mut lane = div()
            .absolute()
            .left(px(TIMELINE_GUTTER))
            .right_0()
            .top(px(top))
            .h(px(TIMELINE_TRACK_HEIGHT))
            .overflow_hidden()
            .border_b_1()
            .border_color(gpui_kit::rgb(0x22222a))
            .on_drag_move(cx.listener(
                move |this, event: &DragMoveEvent<TimelineDrag>, _window, cx| {
                    let position = event.event.position;
                    if !event.bounds.contains(&position) {
                        return;
                    }
                    // 抓取偏移只在 `on_drag` 的构造器里写得到，这里读出来还原片段左边缘。
                    let (clip, grab) = {
                        let drag = event.drag(cx);
                        (drag.clip, drag.grab.get())
                    };
                    let Some(project) = this.project.as_ref() else {
                        return;
                    };
                    let fps = project.sequence.fps;
                    let lane_x = f32::from(position.x) - f32::from(event.bounds.origin.x);
                    let seconds = this.timeline.time_at(lane_x - grab).max(0.0);
                    let start = snap_to_frame(Tick::from_secs(seconds), fps);
                    this.set_timeline_drop(
                        TimelineDrop {
                            clip,
                            to_track: track_id,
                            start,
                        },
                        cx,
                    );
                },
            ))
            .on_drop(cx.listener(move |this, drag: &TimelineDrag, _window, cx| {
                this.drop_timeline_clip(drag.clip, cx);
            }));

        for clip in &track.clips {
            let start = clip.start.secs();
            let end = clip.end().secs();
            if end <= view.scroll() || start >= visible_end {
                continue;
            }
            let left = view.x_of(start).max(0.0);
            let right = view.x_of(end).min(lane_width);
            let width = (right - left).max(2.0);
            let name = if clip.name.is_empty() {
                project
                    .asset(clip.asset_id)
                    .map(|asset| asset.display_name())
                    .unwrap_or_else(|| "片段".to_string())
            } else {
                clip.name.clone()
            };
            let clip_id = clip.id;
            let ghost_name: SharedString = name.clone().into();
            let grab = Rc::new(Cell::new(0.0f32));
            let drag_grab = grab.clone();

            lane = lane.child(
                div()
                    // `on_drag` 挂在有身份的元素上：命中测试要能认出「拖的是哪一个」。
                    .id(format!("timeline-clip-{}", clip_id.raw()))
                    .absolute()
                    .left(px(left))
                    .top(px(4.0))
                    .bottom(px(4.0))
                    .w(px(width))
                    .rounded(px(4.0))
                    .overflow_hidden()
                    .px(px(5.0))
                    .flex()
                    .items_center()
                    .bg(clip_color(track.kind))
                    // 关掉的轨道整体压暗：不改颜色本身，缩放很小的时候也能看出它不参与输出。
                    .opacity(if track.muted { 0.4 } else { 1.0 })
                    .when(width > 44.0, |this| {
                        this.text_size(px(11.0))
                            .text_color(gpui_kit::rgba(0xffffffee))
                            .whitespace_nowrap()
                            .child(name)
                    })
                    // 拖到别的同类型轨道；落回同一条轨道就是改起点。
                    .on_drag(
                        TimelineDrag {
                            clip: clip_id,
                            grab,
                        },
                        move |_drag, offset, _window, cx| {
                            drag_grab.set(f32::from(offset.x));
                            cx.new(|_| TimelineGhost {
                                name: ghost_name.clone(),
                            })
                        },
                    ),
            );
        }

        // 拖动预览：贴着落点画一个半透明框，松手之前就能看出会落到哪。
        if let Some(drop) = self.timeline_drop
            && drop.to_track == track_id
            && let Some(clip) = project.sequence.clip(drop.clip)
        {
            let left = view.x_of(drop.start.secs()).max(0.0);
            let right = view
                .x_of((drop.start + clip.duration()).secs())
                .min(lane_width);
            lane = lane.child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(4.0))
                    .bottom(px(4.0))
                    .w(px((right - left).max(2.0)))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(ui::accent())
                    .bg(gpui_kit::rgba(0x3f6fd855)),
            );
        }

        lane.into_any_element()
    }

    /// 左侧名称列：轨道类型、名字与两个状态开关。
    pub(super) fn render_timeline_label(
        &self,
        track: &crate::timeline::Track,
        top: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let track_id = track.id;

        // 视频 / 字幕轨的「关掉」是隐藏，音频轨是静音：同一个状态，两种说法。
        let (mute_icon, mute_tip) = match track.kind {
            crate::timeline::TrackKind::Audio => (
                if track.muted {
                    IconName::VolumeX
                } else {
                    IconName::Volume2
                },
                if track.muted {
                    "取消静音"
                } else {
                    "静音这一轨"
                },
            ),
            _ => (
                if track.muted {
                    IconName::EyeOff
                } else {
                    IconName::Eye
                },
                if track.muted {
                    "显示这一轨"
                } else {
                    "隐藏这一轨"
                },
            ),
        };
        let (lock_icon, lock_tip) = if track.locked {
            (IconName::Lock, "解锁这一轨")
        } else {
            (IconName::LockOpen, "锁定这一轨（禁止编辑）")
        };

        div()
            .absolute()
            .left_0()
            .top(px(top))
            .w(px(TIMELINE_GUTTER))
            .h(px(TIMELINE_TRACK_HEIGHT))
            .flex()
            .flex_col()
            .justify_center()
            .gap(px(2.0))
            .px(px(8.0))
            .border_b_1()
            .border_color(gpui_kit::rgb(0x22222a))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .size(px(7.0))
                            .rounded_full()
                            .bg(clip_color(track.kind)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(11.0))
                            // 关掉的轨道用更淡的颜色，一眼能从名字列看出它不参与输出。
                            .text_color(if track.muted {
                                text_faint()
                            } else {
                                text_muted()
                            })
                            .child(track.name.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .child(ui::icon_toggle_small(
                        format!("track-mute-{}", track_id.raw()),
                        mute_icon,
                        track.muted,
                        mute_tip,
                        cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.toggle_track_muted(track_id, cx);
                        }),
                    ))
                    .child(ui::icon_toggle_small(
                        format!("track-lock-{}", track_id.raw()),
                        lock_icon,
                        track.locked,
                        lock_tip,
                        cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.toggle_track_locked(track_id, cx);
                        }),
                    )),
            )
            .into_any_element()
    }

    /// 切换轨道的静音 / 隐藏。锁定不影响它——锁的是编辑，不是输出。
    pub(super) fn toggle_track_muted(
        &mut self,
        track: crate::timeline::Id,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let Some(track) = project.sequence.track_mut(track) else {
            return;
        };
        track.muted = !track.muted;
        let muted = track.muted;
        let kind = track.kind;
        let name = track.name.clone();

        let action = match (kind, muted) {
            (crate::timeline::TrackKind::Audio, true) => "静音",
            (crate::timeline::TrackKind::Audio, false) => "恢复声音",
            (_, true) => "隐藏",
            (_, false) => "显示",
        };
        self.flash(format!("轨道「{name}」已{action}"));
        cx.notify();
    }

    /// 切换轨道锁定。锁定的轨道拒绝一切编辑（校验在 `timeline::ops`）。
    pub(super) fn toggle_track_locked(
        &mut self,
        track: crate::timeline::Id,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let Some(track) = project.sequence.track_mut(track) else {
            return;
        };
        track.locked = !track.locked;
        let locked = track.locked;
        let name = track.name.clone();

        self.flash(format!(
            "轨道「{name}」已{}",
            if locked { "锁定" } else { "解锁" }
        ));
        cx.notify();
    }

    /// 记下落点。相同落点直接返回，拖动时指针每移动一个像素都重绘整窗太浪费。
    fn set_timeline_drop(&mut self, drop: TimelineDrop, cx: &mut Context<Self>) {
        if self.timeline_drop == Some(drop) {
            return;
        }
        self.timeline_drop = Some(drop);
        cx.notify();
    }

    /// 松手：把落点交给 `timeline::ops`，重叠 / 锁定 / 轨道类型这些校验都在那边。
    fn drop_timeline_clip(&mut self, clip: Id, cx: &mut Context<Self>) {
        let Some(drop) = self.timeline_drop.take() else {
            return;
        };
        if drop.clip != clip || self.project.is_none() {
            cx.notify();
            return;
        }

        let op = EditOp::Move {
            clip,
            to_track: drop.to_track,
            start: drop.start,
        };
        let outcome = match self.project.as_ref() {
            Some(project) => apply_edit(&project.sequence, &op),
            None => return,
        };
        match outcome {
            Ok(sequence) => {
                if let Some(project) = self.project.as_mut() {
                    project.sequence = sequence;
                }
                self.flash("已移动片段");
            }
            Err(error) => self.flash(format!("移动失败：{error}")),
        }
        self.poke();
        cx.notify();
    }
}

/// 片段底色：视频蓝、音频绿、字幕橙。整块颜色比图标更容易在缩放很小时认出来。
fn clip_color(kind: crate::timeline::TrackKind) -> gpui_kit::Rgba {
    match kind {
        crate::timeline::TrackKind::Video => gpui_kit::rgba(0x3f6fd8e6),
        crate::timeline::TrackKind::Audio => gpui_kit::rgba(0x2f8f5be6),
        crate::timeline::TrackKind::Subtitle => gpui_kit::rgba(0xb07a2ee6),
    }
}

/// 落点对齐到帧：像素换算出来的时间是连续值，不对齐会得到「半帧」的起点，
/// 导出时那一帧要么重复要么丢掉。
fn snap_to_frame(tick: Tick, fps: Rational) -> Tick {
    Tick::from_frames(tick.round_frames(fps), fps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 落点吸附到最近的帧() {
        let fps = Rational::FPS_25;
        assert_eq!(
            snap_to_frame(Tick::from_millis(10), fps),
            Tick::ZERO,
            "0.01 秒贴着第 0 帧"
        );
        assert_eq!(
            snap_to_frame(Tick::from_millis(30), fps),
            Tick::from_millis(40),
            "0.03 秒更靠近第 1 帧"
        );
        assert_eq!(
            snap_to_frame(Tick::from_millis(49), fps),
            Tick::from_millis(40)
        );
    }
}
