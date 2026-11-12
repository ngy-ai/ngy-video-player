//! 文件信息浮层。

use crate::app::*;

impl PlayerApp {
    /// 「查看格式」面板：文件头 + 探测结果（容器、时长、大小、每条轨道）。
    pub(in crate::app) fn render_file_info(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
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
