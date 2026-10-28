//! 浮层：OSD、截图卡片、错误提示、条目菜单、文件信息。

use super::*;

impl PlayerApp {
    pub(super) fn render_osd(&self, top: Pixels, panel: f32) -> Option<impl IntoElement> {
        let (lines, _) = self.osd.as_ref()?;
        Some(
            div()
                .absolute()
                .left(px(0.0))
                // 提示条在画面上居中，别压到侧边栏上。
                .right(px(panel))
                .top(top)
                .flex()
                .justify_center()
                .child(
                    div()
                        .px(px(14.0))
                        .py(px(8.0))
                        .rounded(px(10.0))
                        .bg(gpui_kit::rgba(0x000000cc))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(2.0))
                        .children(lines.iter().map(|line| {
                            div()
                                .text_size(px(14.0))
                                .text_color(text())
                                .child(line.clone())
                        })),
                ),
        )
    }

    /// 截图卡片：预览 + 复制 / 打开 / 定位。
    ///
    /// 放左上角：画面底部是字幕、控件也在那儿，卡片压过去会挡住用户真正在看的东西。
    pub(super) fn render_screenshot_card(
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

    pub(super) fn render_error(
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
                        .text_size(px(13.0))
                        .text_color(ui::danger())
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(text_muted())
                        .child(message),
                ),
        )
    }

    /// 行右键菜单：贴在鼠标位置，三项都是对「这一个文件」的操作。
    pub(super) fn render_item_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let menu = self.item_menu.as_ref()?;
        let index = menu.index;
        let reveal_path = menu.path.clone();
        let info_path = menu.path.clone();

        Some(
            div()
                .id("item-menu")
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
                // 菜单上的点击统一吃掉：一是别漏到画面去切播放状态，二是别让
                // 根节点的「点别处收菜单」把菜单在 mouse-down 阶段就拆掉，
                // 否则 mouse-up 时元素已经不在，菜单项永远点不中。
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
                    "item-menu-delete",
                    IconName::Trash,
                    "删除",
                    ui::danger(),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.item_menu = None;
                        this.remove_items(&[index], cx);
                    }),
                ))
                .child(item_menu_entry(
                    "item-menu-reveal",
                    IconName::FolderOpen,
                    "定位文件",
                    text(),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.reveal_item(reveal_path.clone(), cx);
                    }),
                ))
                .child(item_menu_entry(
                    "item-menu-info",
                    IconName::Info,
                    "查看格式",
                    text(),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.show_file_info(info_path.clone(), cx);
                    }),
                )),
        )
    }

    /// 「查看格式」面板：文件头 + 探测结果（容器、时长、大小、每条轨道）。
    pub(super) fn render_file_info(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let panel = self.file_info.as_ref()?;
        let name = panel
            .path
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| panel.path.display().to_string());

        let body: AnyElement = match &panel.state {
            InfoState::Loading => {
                ui::label("正在读取格式信息…", 13.0, text_muted()).into_any_element()
            }
            InfoState::Failed(message) => div()
                .text_size(px(13.0))
                .text_color(ui::danger())
                .child(message.clone())
                .into_any_element(),
            InfoState::Ready(info) => render_media_info(info),
        };

        Some(
            div()
                .id("file-info")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui_kit::rgba(0x000000b8))
                // 面板是模态的：点它不该穿透去暂停视频。
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .child(
                    div()
                        .w(px(560.0))
                        // 窗口被拖窄时面板不能顶出画面，否则右上角的关闭按钮会落到
                        // 窗口外，用户只能按 Esc 关。
                        .max_w(gpui_kit::relative(0.92))
                        .p(px(20.0))
                        .rounded(px(14.0))
                        .bg(surface())
                        .border_1()
                        .border_color(gpui_kit::rgb(0x2c2c34))
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(12.0))
                                .child(
                                    div()
                                        .flex_1()
                                        // 课程视频的文件名常常一整行放不下；没有 min_w(0)
                                        // 时这一列会被文字撑宽，把右边的关闭按钮顶出面板。
                                        .min_w(px(0.0))
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.0))
                                        .child(
                                            div()
                                                .text_size(px(15.0))
                                                .text_color(text())
                                                .child(ui::ellipsis(name)),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.0))
                                                .text_color(text_faint())
                                                .child(ui::ellipsis(
                                                    panel.path.display().to_string(),
                                                )),
                                        ),
                                )
                                .child(icon_button(
                                    "file-info-close",
                                    IconName::X,
                                    false,
                                    false,
                                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                                        this.file_info = None;
                                        cx.notify();
                                    }),
                                )),
                        )
                        .child(
                            div()
                                .id("file-info-body")
                                .max_h(px(430.0))
                                .overflow_y_scroll()
                                .child(body),
                        ),
                ),
        )
    }
}
