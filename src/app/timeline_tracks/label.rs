//! 轨道的名称列与两个状态开关（静音 / 隐藏、锁定）。

use super::*;

impl PlayerApp {
    /// 左侧名称列：轨道类型、名字与两个状态开关。
    pub(crate) fn render_timeline_label(
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
        let delete_tip = format!("删除这条{}轨「{}」", track.kind.label(), track.name);

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
                    ))
                    // 删轨放在轨道头上：视频轨与音频轨同一个入口，不用先选中某一段。
                    .child(ui::icon_toggle_small(
                        format!("track-delete-{}", track_id.raw()),
                        IconName::Trash,
                        false,
                        delete_tip,
                        cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.delete_track(track_id, cx);
                        }),
                    )),
            )
            .into_any_element()
    }

    /// 切换轨道的静音 / 隐藏。锁定不影响它——锁的是编辑，不是输出。
    pub(crate) fn toggle_track_muted(
        &mut self,
        track: crate::timeline::Id,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let Some(track) = project.sequence_mut().track_mut(track) else {
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
        let message = format!("轨道「{name}」已{action}");
        self.remember_history(message.clone());
        self.flash(message);
        cx.notify();
    }

    /// 切换轨道锁定。锁定的轨道拒绝一切编辑（校验在 `timeline::ops`）。
    pub(crate) fn toggle_track_locked(
        &mut self,
        track: crate::timeline::Id,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let Some(track) = project.sequence_mut().track_mut(track) else {
            return;
        };
        track.locked = !track.locked;
        let locked = track.locked;
        let name = track.name.clone();

        let message = format!("轨道「{name}」已{}", if locked { "锁定" } else { "解锁" });
        self.remember_history(message.clone());
        self.flash(message);
        cx.notify();
    }
}
