//! 时间线标题条：序列信息、播放键与一排编辑 / 导出按钮。
//!
//! 单独成文件只因为 `timeline_view.rs` 已经接近 700 行上限：这里是纯绘制，
//! 状态全在 `PlayerApp` 上，搬出来不改变任何行为。

use super::*;

impl PlayerApp {
    /// 标题条：左边序列名与帧率，右边播放、撤销 / 重做 / 历史、切割、加轨与导出。
    pub(super) fn render_timeline_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(project) = &self.project else {
            return div().into_any_element();
        };
        let sequence = &project.sequence;
        let duration = sequence.duration();
        let playhead = project.ui.playhead.secs();
        // 能用才点亮：灰色按钮比点了没反应更容易看懂。
        let can_undo = self.history.as_ref().is_some_and(UndoHistory::can_undo);
        let can_redo = self.history.as_ref().is_some_and(UndoHistory::can_redo);

        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(TIMELINE_HEADER))
            .flex()
            .items_center()
            .justify_between()
            .px(px(10.0))
            .border_b_1()
            .border_color(gpui_kit::rgb(0x26262e))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(13.0))
                    .text_color(text())
                    .child(sequence.name.clone())
                    // 未保存标记：整个工程只有这一处提示，别让它太安静。
                    .when(self.project_modified(), |this| {
                        this.child(div().text_color(ui::accent()).child("*"))
                    })
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text_faint())
                            .child(format!("{} fps", sequence.fps)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .text_size(px(11.0))
                    .text_color(text_muted())
                    // 控制条在工程模式下整条不画，播放键只能放在这里；不然空格是
                    // 唯一入口，新用户会以为时间线只能拖着看。
                    .child(ui::icon_toggle_small(
                        "timeline-play",
                        if self.timeline_playing {
                            IconName::Pause
                        } else {
                            IconName::Play
                        },
                        self.timeline_playing,
                        if self.timeline_playing {
                            "暂停预览（空格）"
                        } else {
                            "播放预览（空格）"
                        },
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.toggle_play(cx);
                        }),
                    ))
                    .child(ui::icon_toggle_small(
                        "timeline-undo",
                        IconName::Undo,
                        can_undo,
                        if cfg!(target_os = "macos") {
                            "撤销（⌘Z）"
                        } else {
                            "撤销（Ctrl+Z）"
                        },
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.undo_edit(cx);
                        }),
                    ))
                    .child(ui::icon_toggle_small(
                        "timeline-redo",
                        IconName::Redo,
                        can_redo,
                        if cfg!(target_os = "macos") {
                            "重做（⇧⌘Z）"
                        } else {
                            "重做（Ctrl+Shift+Z）"
                        },
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.redo_edit(cx);
                        }),
                    ))
                    .child(ui::icon_toggle_small(
                        "timeline-history",
                        IconName::List,
                        self.show_history,
                        "编辑历史侧栏（点一步直接回退 / 前进）",
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.open_history_panel(cx);
                        }),
                    ))
                    .child(ui::icon_toggle_small(
                        "timeline-split",
                        IconName::Scissors,
                        false,
                        if cfg!(target_os = "macos") {
                            "在播放头处切割（⌘K）"
                        } else {
                            "在播放头处切割（Ctrl+K）"
                        },
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.split_at_playhead(cx);
                        }),
                    ))
                    // 多轨是导出的能力，得先能建出来。两颗按钮各自加一条同类轨。
                    .child(ui::icon_toggle_small(
                        "timeline-add-video",
                        IconName::Plus,
                        false,
                        "加一条视频轨",
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.add_track(crate::timeline::TrackKind::Video, cx);
                        }),
                    ))
                    .child(ui::icon_toggle_small(
                        "timeline-add-audio",
                        IconName::Volume2,
                        false,
                        "加一条音频轨",
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.add_track(crate::timeline::TrackKind::Audio, cx);
                        }),
                    ))
                    .child(ui::icon_toggle_small(
                        "timeline-export",
                        IconName::FileDown,
                        false,
                        "导出为视频文件",
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.export_sequence(window, cx);
                        }),
                    ))
                    // 还在排队的条数：不然连着点几次，看不出后面还压着活。
                    .when_some(self.export_queue_note(), |this, note| this.child(note))
                    // 导出跑起来之后原地显示进度与取消，不另开一个面板。
                    .when_some(self.export_progress, |this, progress| {
                        this.child(format!("导出 {:.0}%", progress * 100.0)).child(
                            ui::icon_toggle_small(
                                "timeline-export-cancel",
                                IconName::X,
                                false,
                                "取消导出",
                                cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    this.cancel_export(cx);
                                }),
                            ),
                        )
                    })
                    // 工作区读数：只有一端时也要说出来。单边入点 / 出点在标尺上
                    // 只是一条线，不给读数的话用户会以为快捷键没生效。
                    .when_some(work_area_label(project), |this, label| {
                        this.child(div().text_color(ui::accent()).child(label))
                    })
                    .child(format_time(duration.secs()))
                    .child(format_time_precise(playhead))
                    .child(format!("{:.1} 像素/秒", self.timeline.zoom())),
            )
            .into_any_element()
    }
}

/// 标题条上的工作区读数。
///
/// 只有一端时说明还在等另一端：区间要两端齐全才算成立，但「已经设了入点」
/// 这件事本身得看得见。
fn work_area_label(project: &Project) -> Option<String> {
    match (project.ui.in_point, project.ui.out_point) {
        (Some(start), Some(end)) if start < end => Some(format!(
            "工作区 {} – {}",
            format_time_precise(start.secs()),
            format_time_precise(end.secs())
        )),
        (Some(start), _) => Some(format!("入点 {}", format_time_precise(start.secs()))),
        (None, Some(end)) => Some(format!("出点 {}", format_time_precise(end.secs()))),
        (None, None) => None,
    }
}
