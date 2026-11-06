//! 编辑历史列表窗口：把撤销栈里的每一步摊开，点一行直接跳到那一步。
//!
//! 撤销 / 重做只挪一格，改错了十几步之前的东西就得一直按 Ctrl+Z；这里把快照
//! 自带的说明列出来，往前点是回退、往后点是前进，跳过去之后重做分支仍然留着。

use super::*;

impl PlayerApp {
    /// 打开历史列表。再点一次入口就收起，和别的对话框开关一致。
    pub(super) fn open_history_dialog(&mut self, cx: &mut Context<Self>) {
        if self.show_history {
            self.close_history_dialog(cx);
            return;
        }
        if self.project.is_none() {
            self.flash("先打开或新建一个工程");
            cx.notify();
            return;
        }

        // 模态互斥：其它对话框都收掉，否则两层全屏遮罩会叠在一起抢键盘。
        self.close_remux_dialog(cx);
        self.close_metadata_dialog(cx);
        self.close_proxy_dialog(cx);
        self.close_scope_dialog(cx);
        self.close_scenes_dialog(cx);
        self.close_multicam_dialog(cx);
        self.close_clip_dialog(cx);
        self.close_convert_dialog(cx);
        self.close_concat_dialog(cx);
        self.close_playlist_import_dialog(cx);
        self.show_settings = false;
        self.file_info = None;
        self.timeline_menu = None;

        self.show_history = true;
        self.menu = None;
        self.menu_bar = None;
        cx.notify();
    }

    pub(super) fn close_history_dialog(&mut self, cx: &mut Context<Self>) {
        if self.show_history {
            self.show_history = false;
            cx.notify();
        }
    }

    pub(super) fn render_history_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.show_history {
            return None;
        }
        let history = self.history.as_ref()?;
        let current = history.current_index();
        let labels: Vec<String> = history.labels().map(str::to_string).collect();
        let total = labels.len();
        let can_undo = history.can_undo();
        let can_redo = history.can_redo();

        let mut rows = div()
            .id("history-step-list")
            .flex()
            .flex_col()
            .gap(px(2.0))
            .max_h(px(300.0))
            .overflow_y_scroll();
        for (index, label) in labels.into_iter().enumerate() {
            let is_current = index == current;
            let state = if is_current {
                "当前"
            } else if index > current {
                "待重做"
            } else {
                "已执行"
            };
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("history-step-{index}")))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(6.0))
                    .cursor(CursorStyle::PointingHand)
                    .when(is_current, |this| this.bg(gpui_kit::rgba(0x5aa9ff1f)))
                    .when(!is_current, |this| {
                        this.hover(|style| style.bg(ui::surface_hover()))
                    })
                    .child(
                        div()
                            .w(px(26.0))
                            .flex_none()
                            .text_size(px(11.0))
                            .text_color(if is_current {
                                ui::accent()
                            } else {
                                text_faint()
                            })
                            .child(format!("{:02}", index + 1)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(12.0))
                            .text_color(if index > current {
                                text_faint()
                            } else {
                                text()
                            })
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(10.0))
                            .text_color(text_faint())
                            .child(state),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        // 模态底下的时间线也接点击，不拦住会顺手把播放头挪走。
                        cx.stop_propagation();
                        this.jump_to_history_step(index, cx);
                    })),
            );
        }

        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(text_muted())
                    .child(format!(
                        "含初始状态共 {} 个，最多保留 {} 步操作。点任意一行即可回退或前进。",
                        total,
                        history.limit()
                    )),
            )
            .child(rows)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(settings_button(
                                "history-undo",
                                "撤销一步",
                                can_undo,
                                cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    this.undo_edit(cx);
                                }),
                            ))
                            .child(settings_button(
                                "history-redo",
                                "重做一步",
                                can_redo,
                                cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    this.redo_edit(cx);
                                }),
                            )),
                    )
                    .child(primary_button(
                        "history-close",
                        "关闭",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.close_history_dialog(cx);
                        }),
                    )),
            );

        Some(modal_card(
            "history-dialog",
            "编辑历史",
            body.into_any_element(),
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.close_history_dialog(cx);
            }),
        ))
    }
}
