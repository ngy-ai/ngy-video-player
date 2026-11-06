//! 编辑历史侧栏：贴在工程模式预览区右侧，点一行直接跳到那一步。
//!
//! 不做模态：历史是「一直看着、随手点一下」的东西，弹窗会把预览连同正在看的
//! 那帧全遮住，也没法边看画面边试几步之前的状态。侧栏与画面并排，宽度统一由
//! `side_panel_width()` 算；底部让开时间线，时间线仍铺满整窗。

use super::*;

impl PlayerApp {
    /// 开关编辑历史侧栏。再点一次入口就收起。
    pub(super) fn open_history_panel(&mut self, cx: &mut Context<Self>) {
        if self.show_history {
            self.close_history_panel(cx);
            return;
        }
        if self.project.is_none() {
            self.flash("先打开或新建一个工程");
            cx.notify();
            return;
        }
        // 侧栏不是模态：别的对话框弹出来时仍压在它上面，不必互相收掉。
        // 工程模式下裁切 / 转换 / 播放列表本来就不画（见 `render`），右侧只此一条。
        self.timeline_menu = None;
        self.menu_bar = None;
        self.show_history = true;
        cx.notify();
    }

    pub(super) fn close_history_panel(&mut self, cx: &mut Context<Self>) {
        if self.show_history {
            self.show_history = false;
            cx.notify();
        }
    }

    /// `bottom` 是时间线高度：侧栏只占预览区右侧，不压住底部时间线。
    pub(super) fn render_history_panel(
        &self,
        top: Pixels,
        width: f32,
        bottom: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.show_history || width <= 0.0 {
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
            .px(px(8.0))
            .flex_1()
            .min_h(px(0.0))
            // 两百步也不该把面板撑出屏幕：列表自己滚，标题和按钮留在两头。
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
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(px(6.0))
                    .cursor(CursorStyle::PointingHand)
                    .when(is_current, |this| this.bg(gpui_kit::rgba(0x5aa9ff1f)))
                    .when(!is_current, |this| {
                        this.hover(|style| style.bg(ui::surface_hover()))
                    })
                    .child(
                        div()
                            .w(px(22.0))
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
                        // 侧栏底下的画面也接点击，不拦住会顺手切换播放 / 暂停。
                        cx.stop_propagation();
                        this.jump_to_history_step(index, cx);
                    })),
            );
        }

        Some(
            div()
                .id("history-panel")
                .absolute()
                .top(top)
                .right_0()
                .bottom(px(bottom))
                .w(px(width))
                .flex()
                .flex_col()
                .bg(gpui_kit::rgba(0x131317e6))
                .border_l_1()
                .border_color(gpui_kit::rgb(0x26262e))
                // 面板上的点击不该漏到画面去（画面那一层负责播放 / 暂停）。
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .pl(px(14.0))
                        .pr(px(6.0))
                        .py(px(8.0))
                        .child(
                            div()
                                .text_size(px(14.0))
                                .text_color(text())
                                .child("编辑历史"),
                        )
                        .child(icon_button(
                            "history-close",
                            IconName::X,
                            false,
                            false,
                            cx.listener(|this, _: &ClickEvent, _window, cx| {
                                this.close_history_panel(cx);
                            }),
                        )),
                )
                .child(
                    div()
                        .px(px(14.0))
                        .pb(px(8.0))
                        .text_size(px(11.0))
                        .text_color(text_faint())
                        .whitespace_normal()
                        .child(format!(
                            "含初始状态共 {total} 个，最多保留 {} 步。点任意一行回退或前进。",
                            history.limit()
                        )),
                )
                .child(rows)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .p(px(10.0))
                        .border_t_1()
                        .border_color(gpui_kit::rgb(0x26262e))
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
                .into_any_element(),
        )
    }
}
