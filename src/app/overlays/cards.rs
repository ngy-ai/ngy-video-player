//! 截图卡片与错误提示：右下角那一摞，以及顶部报错条。

use crate::app::*;

impl PlayerApp {
    /// 截图卡片：预览 + 复制 / 打开 / 定位。
    ///
    /// 放左上角：画面底部是字幕、控件也在那儿，卡片压过去会挡住用户真正在看的东西。
    pub(in crate::app) fn render_screenshot_card(
        &self,
        top: Pixels,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let card = self.screenshot_card.as_ref()?;

        let preview = match &card.thumb {
            ScreenshotThumb::Ready(thumb) => img(thumb.clone()).size_full().into_any_element(),
            ScreenshotThumb::Loading => placeholder("正在读取预览…"),
            ScreenshotThumb::Failed => placeholder("预览不可用"),
        };

        Some(
            div()
                .id("screenshot-card")
                .absolute()
                .left(px(24.0))
                .top(top)
                .w(px(SCREENSHOT_CARD_WIDTH))
                .rounded(px(12.0))
                .bg(surface())
                .border_1()
                .border_color(gpui_kit::rgb(0x2c2c34))
                .shadow_lg()
                .overflow_hidden()
                // 卡片浮在画面上：点击和按下都在这里吃掉，否则会漏到底下触发暂停。
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_this, _: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                    }),
                )
                .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                    if let Some(card) = this.screenshot_card.as_mut() {
                        card.hovered = *hovered;
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(SCREENSHOT_PREVIEW_HEIGHT))
                        .bg(gpui_kit::rgb(0x08080a))
                        .child(preview)
                        .child(
                            div()
                                .absolute()
                                .top(px(6.0))
                                .right(px(6.0))
                                .child(icon_button(
                                    "screenshot-card-close",
                                    IconName::X,
                                    false,
                                    false,
                                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                                        this.clear_screenshot_card();
                                        cx.notify();
                                    }),
                                )),
                        ),
                )
                .child(div().h(px(1.0)).w_full().bg(gpui_kit::rgb(0x2c2c34)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(screenshot_card_action(
                            "screenshot-card-copy",
                            IconName::Copy,
                            platform::clipboard::COPY_LABEL,
                            cx.listener(|this, _: &ClickEvent, _window, cx| {
                                this.copy_screenshot(cx)
                            }),
                        ))
                        .child(card_divider())
                        .child(screenshot_card_action(
                            "screenshot-card-open",
                            IconName::ExternalLink,
                            "打开",
                            cx.listener(|this, _: &ClickEvent, _window, cx| {
                                this.open_screenshot(cx)
                            }),
                        ))
                        .child(card_divider())
                        .child(screenshot_card_action(
                            "screenshot-card-reveal",
                            IconName::FolderOpen,
                            "定位",
                            cx.listener(|this, _: &ClickEvent, _window, cx| {
                                this.reveal_screenshot(cx)
                            }),
                        )),
                ),
        )
    }

    pub(in crate::app) fn render_error(
        &self,
        panel: f32,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let (title, message) = match (&self.error, &self.startup_failure) {
            (Some(banner), _) => (banner.title, banner.body.clone()),
            (None, Some(failure)) => ("播放出错", failure.clone()),
            (None, None) => return None,
        };
        Some(
            div()
                .id("error")
                .absolute()
                .left(px(24.0))
                .right(px(24.0 + panel))
                .bottom(px(96.0))
                .p(px(14.0))
                .rounded(px(10.0))
                .bg(gpui_kit::rgba(0x2a1414f2))
                .border_1()
                .border_color(gpui_kit::rgb(0x5a2424))
                // 点横幅（比如想选中文字）不该顺手暂停视频。
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .text_size(px(13.0))
                                .text_color(ui::danger())
                                .child(title),
                        )
                        .child(icon_button(
                            "error-close",
                            IconName::X,
                            false,
                            false,
                            cx.listener(|this, _: &ClickEvent, _window, cx| {
                                // 启动失败那条也挂在这块横幅上，两个一起收掉。
                                this.error = None;
                                this.startup_failure = None;
                                cx.notify();
                            }),
                        )),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(text_muted())
                        .child(message),
                ),
        )
    }

    /// 分轨导出的持续横幅：整批进度 + 当前这条的进度条 + 取消。
    ///
    /// 压在预览区底部、时间线之上（工程模式下 `bottom` 是时间线高度），不占布局。
    pub(in crate::app) fn render_stem_banner(
        &self,
        panel: f32,
        bottom: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let state = self.stem_export.as_ref()?;
        let progress = state.progress.clamp(0.0, 1.0);
        Some(
            div()
                .id("stem-export-banner")
                .absolute()
                .left(px(24.0))
                .right(px(24.0 + panel))
                .bottom(px(bottom + 16.0))
                .p(px(10.0))
                .rounded(px(8.0))
                .bg(gpui_kit::rgba(0x1414_1cf2))
                .border_1()
                .border_color(gpui_kit::rgb(0x2c2c34))
                // 浮层上的点击不该顺手暂停画面 / 选中时间线片段。
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .flex()
                .items_center()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(gpui_kit::rgb(0xe8e8ee))
                        .child(state.label()),
                )
                .child(
                    div()
                        .flex_1()
                        .h(px(4.0))
                        .rounded(px(2.0))
                        .bg(ui::track())
                        .child(
                            div()
                                .h_full()
                                .w(gpui_kit::relative(progress))
                                .rounded(px(2.0))
                                .bg(ui::accent()),
                        ),
                )
                .child(icon_button(
                    "stem-export-cancel",
                    IconName::X,
                    false,
                    false,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.cancel_stem_export(cx);
                    }),
                ))
                .into_any_element(),
        )
    }
}
