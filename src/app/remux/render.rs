use super::super::*;
use super::dialog::{RemuxDialog, RemuxState, hint, labels};
use crate::remux::TrackAction;
use gpui_kit::component::input::Input;
use std::path::Path;

impl PlayerApp {
    /// 对话框本体。放在模态层最上面，遮住底下的视频与菜单。
    pub(in crate::app) fn render_remux_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.remux.as_ref()?;
        let body = match &dialog.state {
            RemuxState::Editing | RemuxState::Failed(_) => self.render_remux_editor(dialog, cx),
            RemuxState::Running { progress } => self.render_remux_progress(*progress, cx),
            RemuxState::Done(path) => self.render_remux_done(path, cx),
        };

        Some(modal_card(
            "remux-dialog",
            dialog.target.title(),
            body,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.close_remux_dialog(cx);
            }),
        ))
    }

    fn render_remux_editor(&self, dialog: &RemuxDialog, cx: &mut Context<Self>) -> AnyElement {
        let name = file_name(&dialog.path);
        let mut body = div().flex().flex_col().gap(px(12.0)).child(
            div()
                .text_size(px(12.0))
                .text_color(text_muted())
                .child(ui::ellipsis(format!(
                    "{name} · 全长 {}",
                    format_time(dialog.duration)
                ))),
        );

        // 失败原因来自 ffmpeg，可能有多行；单独一块红字，别和按钮混在一起。
        if let RemuxState::Failed(message) = &dialog.state {
            body = body.child(
                div()
                    .p(px(10.0))
                    .rounded(px(8.0))
                    .bg(gpui_kit::rgba(0x2a1414f2))
                    .text_size(px(11.0))
                    .text_color(ui::danger())
                    .whitespace_normal()
                    .child(message.clone()),
            );
        }

        body = body
            .child(self.render_remux_section(dialog, cx))
            .child(div().h(px(1.0)).w_full().bg(gpui_kit::rgb(0x2c2c34)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.0))
                    .child(settings_button(
                        "remux-cancel",
                        "取消",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.close_remux_dialog(cx);
                        }),
                    ))
                    .child(primary_button(
                        "remux-start",
                        "开始编辑",
                        dialog.changed(cx),
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.start_remux(cx);
                        }),
                    )),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(text_faint())
                    .whitespace_normal()
                    .child(hint(dialog.target)),
            );
        body.into_any_element()
    }

    /// 当前这一路的现状与三个按钮：选文件替换 / 删除 / 恢复原样。
    fn render_remux_section(&self, dialog: &RemuxDialog, cx: &mut Context<Self>) -> AnyElement {
        let (pick_label, remove_label, keep_label) = labels(dialog.target);
        let (status, color) = match &dialog.action {
            TrackAction::Keep if dialog.builtin.is_empty() => {
                ("没有内置轨道".to_string(), text_faint())
            }
            TrackAction::Keep => (
                format!(
                    "内置 {} 条：{}",
                    dialog.builtin.len(),
                    dialog.builtin.join("、")
                ),
                text_muted(),
            ),
            TrackAction::Replace(path) => (format!("将替换为：{}", file_name(path)), ui::accent()),
            TrackAction::Remove => ("将删除全部同类轨道".to_string(), ui::danger()),
        };

        let pick = cx.listener(|this, _: &ClickEvent, window, cx| {
            this.pick_remux_source(window, cx);
        });
        let remove = cx.listener(|this, _: &ClickEvent, _window, cx| {
            this.set_remux_action(TrackAction::Remove, cx);
        });
        let keep = cx.listener(|this, _: &ClickEvent, _window, cx| {
            this.set_remux_action(TrackAction::Keep, cx);
        });

        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.0))
                            .text_color(text_muted())
                            .child(dialog.target.label()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(12.0))
                            .text_color(color)
                            .child(ui::ellipsis(status)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(settings_button("remux-pick", pick_label, true, pick))
                    .child(settings_button(
                        "remux-remove",
                        remove_label,
                        !matches!(dialog.action, TrackAction::Remove),
                        remove,
                    ))
                    // 「恢复」只在改过之后才有意义，没改过时多一个按钮只是噪音。
                    .when(!dialog.action.is_keep(), |this| {
                        this.child(settings_button("remux-keep", keep_label, true, keep))
                    }),
            )
            // 「只导出音轨」是另一件事：不编辑轨道，而是把第一路音频原样复制成 mka。
            // 摆在同一个对话框里，是因为用户想的都是「这条音轨要怎么办」。
            // 语言与默认轨只在「保留原轨」时可改：换上新轨之后输出里只剩一条，
            // 语言已经由 `-metadata:s:a:0` 那一路负责。
            .when(dialog.action.is_keep() && !dialog.rows.is_empty(), |this| {
                this.child(render_track_tags(dialog, cx))
            })
            .when(dialog.target == EditTarget::Audio, |this| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(settings_button(
                            "remux-extract-audio",
                            "只导出音轨",
                            !dialog.builtin.is_empty(),
                            cx.listener(|this, _: &ClickEvent, _window, cx| {
                                this.extract_audio(cx);
                            }),
                        ))
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(text_faint())
                                .child("原样复制第一路音频，输出同目录 mka"),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_remux_progress(&self, progress: f32, cx: &mut Context<Self>) -> AnyElement {
        let percent = (progress * 100.0).round().clamp(0.0, 100.0) as i32;
        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(text_muted())
                    .child(format!("正在写入新文件… {percent}%")),
            )
            .child(
                div()
                    .h(px(6.0))
                    .w_full()
                    .rounded_full()
                    .bg(ui::track())
                    .child(
                        div()
                            .h_full()
                            .rounded_full()
                            .bg(ui::accent())
                            .w(gpui_kit::relative(progress.clamp(0.0, 1.0))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .child(settings_button(
                        "remux-abort",
                        "取消",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            // 关闭即取消：任务线程负责杀掉 ffmpeg 并删掉半成品。
                            this.close_remux_dialog(cx);
                        }),
                    )),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(text_faint())
                    .child("编辑在后台进行，关掉对话框或按 Esc 会取消并删除半成品。"),
            )
            .into_any_element()
    }

    fn render_remux_done(&self, path: &Path, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(text())
                    .child("新文件已保存"),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_muted())
                    .whitespace_normal()
                    .child(path.display().to_string()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.0))
                    .child(settings_button(
                        "remux-reveal",
                        "定位",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.remux_reveal_result(cx);
                        }),
                    ))
                    .child(settings_button(
                        "remux-open",
                        "打开",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.remux_open_result(cx);
                        }),
                    ))
                    .child(primary_button(
                        "remux-finish",
                        "完成",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.close_remux_dialog(cx);
                        }),
                    )),
            )
            .into_any_element()
    }
}

/// 逐条轨道的语言与默认标记。
///
/// 排在「保留 / 替换 / 删除」之后：只有决定保留时这一块才出现，
/// 顺序上也正好是「先决定要哪条，再决定它叫什么」。
fn render_track_tags(dialog: &RemuxDialog, cx: &mut Context<PlayerApp>) -> AnyElement {
    let mut list = div()
        .id("remux-track-tags")
        .flex()
        .flex_col()
        .gap(px(6.0))
        .max_h(px(200.0))
        .overflow_y_scroll();

    for (index, row) in dialog.rows.iter().enumerate() {
        let default = row.default;
        list = list.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(11.0))
                        .text_color(text_muted())
                        .child(ui::ellipsis(row.label.clone())),
                )
                .child(
                    div().w(px(76.0)).flex_none().child(
                        Input::new(&row.language)
                            .id(SharedString::from(format!("remux-lang-{index}")))
                            .w_full(),
                    ),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("remux-default-{index}")))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .h(px(24.0))
                        .px(px(8.0))
                        .rounded(px(6.0))
                        .text_size(px(11.0))
                        .cursor(CursorStyle::PointingHand)
                        .when(default, |this| {
                            this.bg(ui::accent()).text_color(gpui_kit::rgb(0x111114))
                        })
                        .when(!default, |this| {
                            this.bg(ui::surface_hover())
                                .text_color(text_muted())
                                .hover(|style| style.bg(ui::surface_active()).text_color(text()))
                        })
                        .child("默认")
                        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            cx.stop_propagation();
                            this.toggle_remux_default(index, cx);
                        })),
                ),
        );
    }

    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(11.0))
                        .text_color(text_muted())
                        .child("语言代码 / 默认轨"),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(text_faint())
                        .child("ISO 639，如 chi、eng"),
                ),
        )
        .child(list)
        .into_any_element()
}
