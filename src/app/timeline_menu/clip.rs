//! 片段右键菜单：切开 / 删除（含波纹）/ 转场 / 效果 / 关键帧 / 变速 / 调色，
//! 以及所在轨道的静音、锁定与删轨。

use super::*;

impl PlayerApp {
    /// 片段菜单：10 行 + 1 条分隔线。
    pub(super) fn render_clip_menu(
        &self,
        menu: ClipMenu,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let project = self.project.as_ref()?;
        let track = project.sequence().track(menu.track)?;
        // 片段已经被删掉（比如刚按过 Delete）时，菜单跟着收掉，不留一张空壳。
        project.sequence().clip(menu.clip)?;

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
            (true, true) => "转场设置…",
            (true, false) => "与左侧片段加转场…",
        };
        let clip_id = menu.clip;
        let origin = menu_origin(menu.position, menu_height(10, 1), window);

        Some(
            div()
                .id("timeline-menu")
                .absolute()
                .left(origin.x)
                .top(origin.y)
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
                        this.delete_selected_clips(false, cx);
                    }),
                ))
                // 波纹删除：删掉之后同轨后面的片段往前补，不留空隙。
                .child(item_menu_entry(
                    "timeline-menu-ripple-delete",
                    IconName::Trash,
                    "波纹删除（后面补齐）",
                    ui::danger(),
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.timeline_menu = None;
                        this.delete_selected_clips(true, cx);
                    }),
                ))
                .child(item_menu_entry(
                    "timeline-menu-transition",
                    IconName::Sparkles,
                    transition_label,
                    if adjacent { text() } else { text_faint() },
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.open_transition_menu(clip_id, menu.position, cx);
                    }),
                ))
                .child(item_menu_entry(
                    "timeline-menu-effects",
                    IconName::SlidersHorizontal,
                    "效果…",
                    text(),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.open_effects_menu(clip_id, menu.position, cx);
                    }),
                ))
                .child(item_menu_entry(
                    "timeline-menu-keys",
                    IconName::Waypoints,
                    "关键帧…",
                    text(),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.open_keys_menu(clip_id, menu.position, cx);
                    }),
                ))
                .child(item_menu_entry(
                    "timeline-menu-speed",
                    IconName::Gauge,
                    "变速 / 定格…",
                    text(),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.open_speed_menu(clip_id, menu.position, cx);
                    }),
                ))
                .child(item_menu_entry(
                    "timeline-menu-color",
                    IconName::Palette,
                    "调色…",
                    text(),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.open_color_menu(clip_id, menu.position, cx);
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
                .child(item_menu_entry(
                    "timeline-menu-delete-track",
                    IconName::Trash,
                    "删除整条轨道（含片段）",
                    ui::danger(),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.timeline_menu = None;
                        this.delete_track(track_id, cx);
                    }),
                ))
                .into_any_element(),
        )
    }
}
