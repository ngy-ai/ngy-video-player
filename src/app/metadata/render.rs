use super::super::*;
use super::dialog::{MetadataDialog, MetadataState};
use gpui_kit::component::input::{Input, InputState};
use std::path::Path;

impl PlayerApp {
    pub(in crate::app) fn render_metadata_dialog(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.metadata.as_ref()?;
        let body = match &dialog.state {
            MetadataState::Editing | MetadataState::Failed(_) => {
                self.render_metadata_editor(dialog, cx)
            }
            MetadataState::Running { progress } => self.render_metadata_progress(*progress, cx),
            MetadataState::Done(path) => self.render_metadata_done(path, cx),
        };
        Some(modal_card(
            "metadata-dialog",
            "章节与元数据",
            body,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.close_metadata_dialog(cx);
            }),
        ))
    }

    fn render_metadata_editor(
        &self,
        dialog: &MetadataDialog,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut body = div().flex().flex_col().gap(px(12.0)).child(
            div()
                .text_size(px(12.0))
                .text_color(text_muted())
                .child(ui::ellipsis(format!(
                    "{} · 全长 {}",
                    file_name(&dialog.path),
                    format_time(dialog.duration)
                ))),
        );

        if let MetadataState::Failed(message) = &dialog.state {
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
            .child(tag_field("metadata-title", "标题", &dialog.title))
            .child(tag_field("metadata-artist", "作者", &dialog.artist))
            .child(tag_field("metadata-comment", "备注", &dialog.comment))
            .child(self.render_metadata_cover(dialog, cx))
            .child(div().h(px(1.0)).w_full().bg(gpui_kit::rgb(0x2c2c34)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(text_muted())
                            .child(format!("章节（{}）", dialog.chapters.len())),
                    )
                    .child(settings_button(
                        "metadata-add-chapter",
                        "在当前位置添加",
                        true,
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.add_metadata_chapter(window, cx);
                        }),
                    )),
            );

        if dialog.chapters.is_empty() {
            body = body.child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_faint())
                    .child("没有章节。播到要标记的位置，点「在当前位置添加」。"),
            );
        } else {
            let mut list = div()
                .id("metadata-chapter-list")
                .flex()
                .flex_col()
                .gap(px(6.0))
                .max_h(px(240.0))
                .overflow_y_scroll();
            for (index, row) in dialog.chapters.iter().enumerate() {
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div().w(px(118.0)).flex_none().child(
                                Input::new(&row.start)
                                    .id(SharedString::from(format!(
                                        "metadata-chapter-start-{index}"
                                    )))
                                    .w_full(),
                            ),
                        )
                        .child(
                            div().flex_1().min_w(px(0.0)).child(
                                Input::new(&row.title)
                                    .id(SharedString::from(format!("metadata-chapter-{index}")))
                                    .w_full(),
                            ),
                        )
                        .child(icon_button(
                            SharedString::from(format!("metadata-chapter-remove-{index}")),
                            IconName::X,
                            false,
                            false,
                            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                this.remove_metadata_chapter(index, cx);
                            }),
                        )),
                );
            }
            body = body.child(list);
        }

        body.child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.0))
                .child(settings_button(
                    "metadata-cancel",
                    "取消",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.close_metadata_dialog(cx);
                    }),
                ))
                .child(primary_button(
                    "metadata-save",
                    "保存为新文件",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.start_metadata(cx);
                    }),
                )),
        )
        .child(
            div()
                .text_size(px(10.0))
                .text_color(text_faint())
                .whitespace_normal()
                .child(
                    "画面与音轨流复制到同目录的新 mkv，原文件不会被改动。\
                     标签留空即删除；不选封面就保留原封面，选了则把新图以附件写入。",
                ),
        )
        .into_any_element()
    }

    /// 封面那一行：当前选的图片 + 选择 / 移除。
    fn render_metadata_cover(&self, dialog: &MetadataDialog, cx: &mut Context<Self>) -> AnyElement {
        let (text, color) = match &dialog.cover {
            Some(path) => (file_name(path), text()),
            None => ("保持原样".to_string(), text_faint()),
        };
        div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .w(px(52.0))
                    .flex_none()
                    .text_size(px(12.0))
                    .text_color(text_muted())
                    .child("封面"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(11.0))
                    .text_color(color)
                    .child(ui::ellipsis(text)),
            )
            .child(settings_button(
                "metadata-cover-pick",
                "选择图片…",
                true,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.pick_metadata_cover(window, cx);
                }),
            ))
            .when(dialog.cover.is_some(), |this| {
                this.child(settings_button(
                    "metadata-cover-clear",
                    "撤销",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.clear_metadata_cover(cx);
                    }),
                ))
            })
            .into_any_element()
    }

    fn render_metadata_progress(&self, progress: f32, cx: &mut Context<Self>) -> AnyElement {
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
                        "metadata-abort",
                        "取消",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.close_metadata_dialog(cx);
                        }),
                    )),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(text_faint())
                    .child("关掉对话框会取消并删除半成品。"),
            )
            .into_any_element()
    }

    fn render_metadata_done(&self, path: &Path, cx: &mut Context<Self>) -> AnyElement {
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
                        "metadata-reveal",
                        "定位",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.metadata_reveal_result(cx);
                        }),
                    ))
                    .child(settings_button(
                        "metadata-open",
                        "打开",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.metadata_open_result(cx);
                        }),
                    ))
                    .child(primary_button(
                        "metadata-finish",
                        "完成",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.close_metadata_dialog(cx);
                        }),
                    )),
            )
            .into_any_element()
    }
}

/// 一行标签：左边固定宽度的小标题，右边是组件库输入框。
fn tag_field(id: &'static str, label: &'static str, input: &Entity<InputState>) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .child(
            div()
                .w(px(52.0))
                .flex_none()
                .text_size(px(12.0))
                .text_color(text_muted())
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .child(Input::new(input).id(id).w_full()),
        )
        .into_any_element()
}
