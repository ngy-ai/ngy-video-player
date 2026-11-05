//! 时间线片段的右键菜单：贴在鼠标位置，作用对象是刚点中的那一段片段。
//!
//! 只做「手上这一段」相关的四件事：在播放头处切开、删除、把所在轨道关掉、锁住。
//! 需要跨轨道批量处理的操作（排序、整体重排）留给菜单栏，这里不堆。
//!
//! 外壳复用播放列表行右键菜单的样式（`widgets::item_menu_entry`）：同一个界面上
//! 的右键菜单长得一样，用户不用重新认一遍。

use super::*;
use crate::timeline::Id;

/// 展开中的时间线菜单。
#[derive(Clone, Copy, Debug)]
pub(super) enum TimelineMenu {
    /// 片段右键：作用对象是点中的那一段与它所在的轨道。
    Clip(ClipMenu),
    /// 标尺右键：整条时间线级别的动作，不指向任何片段。
    Ruler { position: Point<Pixels> },
}

/// 片段菜单的上下文：右键点中的那一段、它所在的轨道与鼠标位置。
#[derive(Clone, Copy, Debug)]
pub(super) struct ClipMenu {
    clip: Id,
    track: Id,
    position: Point<Pixels>,
}

impl PlayerApp {
    /// 在片段上右键：先选中它（多选里已经含有时不动），再在鼠标位置弹菜单。
    pub(super) fn open_timeline_menu(
        &mut self,
        clip: Id,
        track: Id,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if let Some(project) = self.project.as_mut() {
            project.ui.selected = vec![clip];
        }
        self.timeline_menu = Some(TimelineMenu::Clip(ClipMenu {
            clip,
            track,
            position,
        }));
        cx.notify();
    }

    /// 标尺上右键：弹出与整条时间线相关的菜单。
    ///
    /// 「清除工作区」放这里当兜底：快捷键那条路要求播放头落回入点 / 出点附近，
    /// 缩得很粗或者手不稳时可能清不掉；这个入口不看播放头。
    pub(super) fn open_ruler_menu(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.project.is_none() {
            return;
        }
        self.timeline_menu = Some(TimelineMenu::Ruler { position });
        cx.notify();
    }

    pub(super) fn render_timeline_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        match self.timeline_menu? {
            TimelineMenu::Clip(menu) => self.render_clip_menu(menu, cx),
            TimelineMenu::Ruler { position } => self.render_ruler_menu(position, cx),
        }
    }

    fn render_clip_menu(&self, menu: ClipMenu, cx: &mut Context<Self>) -> Option<AnyElement> {
        let project = self.project.as_ref()?;
        let track = project.sequence.track(menu.track)?;
        // 片段已经被删掉（比如刚按过 Delete）时，菜单跟着收掉，不留一张空壳。
        project.sequence.clip(menu.clip)?;

        let (mute_label, lock_label) = toggle_labels(track.kind, track.muted, track.locked);
        // 图标跟着状态走：亮着的是「当前状态」，点下去变成另一个。
        let mute_icon = match (track.kind, track.muted) {
            (crate::timeline::TrackKind::Audio, false) => IconName::VolumeX,
            (crate::timeline::TrackKind::Audio, true) => IconName::Volume2,
            (_, false) => IconName::EyeOff,
            (_, true) => IconName::Eye,
        };
        let lock_icon = if track.locked {
            IconName::LockOpen
        } else {
            IconName::Lock
        };
        let track_id = menu.track;

        // 转场只对「紧挨着的同轨前一段」成立，文案得照实说，不能让用户点了才发现。
        let index = track.clip_index(menu.clip).unwrap_or(0);
        let adjacent = index > 0 && track.clips[index - 1].end() == track.clips[index].start;
        let has_transition = adjacent
            && track.transitions.iter().any(|transition| {
                transition.before == track.clips[index - 1].id && transition.after == menu.clip
            });
        let transition_label = match (adjacent, has_transition) {
            (false, _) => "左边没有紧挨着的片段",
            (true, true) => "去掉左侧转场",
            (true, false) => "与左侧片段加转场",
        };
        let clip_id = menu.clip;

        Some(
            div()
                .id("timeline-menu")
                .absolute()
                .left(menu.position.x)
                .top(menu.position.y)
                .w(px(ITEM_MENU_WIDTH))
                .py(px(6.0))
                .rounded(px(10.0))
                .bg(surface())
                .border_1()
                .border_color(gpui_kit::rgb(0x2c2c34))
                .shadow_lg()
                // 菜单上的点击统一吃掉：别漏到时间线（点空白会取消选中），也别让
                // 根节点的「点别处收菜单」在 mouse-down 阶段就把它拆掉。
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_this, _: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                    }),
                )
                .child(item_menu_entry(
                    "timeline-menu-split",
                    IconName::Scissors,
                    "在播放头处切开",
                    text(),
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.timeline_menu = None;
                        this.split_at_playhead(cx);
                    }),
                ))
                .child(item_menu_entry(
                    "timeline-menu-delete",
                    IconName::Trash,
                    "删除片段",
                    ui::danger(),
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.timeline_menu = None;
                        this.delete_selected_clips(cx);
                    }),
                ))
                .child(item_menu_entry(
                    "timeline-menu-transition",
                    IconName::Sparkles,
                    transition_label,
                    if adjacent { text() } else { text_faint() },
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.timeline_menu = None;
                        this.toggle_transition(clip_id, cx);
                    }),
                ))
                .child(
                    div()
                        .mx(px(8.0))
                        .my(px(4.0))
                        .h(px(1.0))
                        .bg(gpui_kit::rgb(0x2c2c34)),
                )
                .child(item_menu_entry(
                    "timeline-menu-mute",
                    mute_icon,
                    mute_label,
                    text(),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.timeline_menu = None;
                        this.toggle_track_muted(track_id, cx);
                    }),
                ))
                .child(item_menu_entry(
                    "timeline-menu-lock",
                    lock_icon,
                    lock_label,
                    text(),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.timeline_menu = None;
                        this.toggle_track_locked(track_id, cx);
                    }),
                ))
                .into_any_element(),
        )
    }

    /// 标尺菜单。外壳与片段菜单一致，只是内容只有时间线级别的那几项。
    fn render_ruler_menu(
        &self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let project = self.project.as_ref()?;
        // 没设过工作区时这一项压暗，点下去给一句说明而不是无声无息。
        let has_range = project.ui.in_point.is_some() || project.ui.out_point.is_some();

        Some(
            div()
                .id("timeline-ruler-menu")
                .absolute()
                .left(position.x)
                .top(position.y)
                .w(px(ITEM_MENU_WIDTH))
                .py(px(6.0))
                .rounded(px(10.0))
                .bg(surface())
                .border_1()
                .border_color(gpui_kit::rgb(0x2c2c34))
                .shadow_lg()
                // 与片段菜单一样：点菜单本身不能漏到下面的标尺去挪播放头。
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_this, _: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                    }),
                )
                .child(item_menu_entry(
                    "timeline-ruler-clear-range",
                    IconName::Trash,
                    "清除工作区",
                    if has_range { text() } else { text_faint() },
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.timeline_menu = None;
                        this.clear_work_area(cx);
                    }),
                ))
                .into_any_element(),
        )
    }
}

/// 菜单上两行开关的文案。
///
/// 同一个 `muted` 字段，音频轨说「静音」、视频 / 字幕轨说「隐藏」——用户看到的
/// 应该是自己那条轨道上的结果，而不是字段名。抽成纯函数，四种组合好逐个钉住。
fn toggle_labels(
    kind: crate::timeline::TrackKind,
    muted: bool,
    locked: bool,
) -> (&'static str, &'static str) {
    let mute = match (kind, muted) {
        (crate::timeline::TrackKind::Audio, false) => "静音这一轨",
        (crate::timeline::TrackKind::Audio, true) => "恢复这一轨的声音",
        (_, false) => "隐藏这一轨",
        (_, true) => "显示这一轨",
    };
    let lock = if locked {
        "解锁这一轨"
    } else {
        "锁定这一轨"
    };
    (mute, lock)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::TrackKind;

    #[test]
    fn 音频说静音视频说隐藏() {
        assert_eq!(
            toggle_labels(TrackKind::Audio, false, false),
            ("静音这一轨", "锁定这一轨")
        );
        assert_eq!(
            toggle_labels(TrackKind::Audio, true, false),
            ("恢复这一轨的声音", "锁定这一轨")
        );
        assert_eq!(
            toggle_labels(TrackKind::Video, false, false),
            ("隐藏这一轨", "锁定这一轨")
        );
        assert_eq!(
            toggle_labels(TrackKind::Video, true, true),
            ("显示这一轨", "解锁这一轨")
        );
    }
}
