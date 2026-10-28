//! 设置窗口的外壳：标题栏、左侧标签列、右侧内容区。

use super::super::*;
use super::tabs::SettingsTab;

/// 卡片内部的分隔线颜色。和菜单、弹出面板用同一条灰线，视觉上是同一套层级。
const LINE: u32 = 0x2c2c34;

impl PlayerApp {
    /// 打开设置窗口。`tab` 为 `None` 时停在上次看的那一页。
    pub(in crate::app) fn open_settings(&mut self, tab: Option<SettingsTab>) {
        if let Some(tab) = tab {
            self.settings_tab = tab;
        }
        self.show_settings = true;
        // 悬浮菜单和模态窗口叠在一起只会互相挡住，先收掉。
        self.menu = None;
        self.menu_bar = None;
        // 关联状态要现场读注册表，不能放进每帧都跑的渲染路径。
        self.refresh_association_status();
    }

    pub(in crate::app) fn set_settings_visible(&mut self, visible: bool) {
        self.show_settings = visible;
    }

    /// 快捷键与按钮共用的入口：已经停在这一页上就收起，否则切过去。
    pub(in crate::app) fn toggle_settings(&mut self, tab: SettingsTab) {
        if self.show_settings && self.settings_tab == tab {
            self.show_settings = false;
        } else {
            self.open_settings(Some(tab));
        }
    }

    pub(in crate::app) fn render_settings(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        if !self.show_settings {
            return None;
        }

        let on_top_supported = platform::topmost::supported(window);
        let body = match self.settings_tab {
            SettingsTab::Picture => self.picture_page(cx),
            SettingsTab::Video => self.video_page(cx),
            SettingsTab::Playback => self.playback_page(on_top_supported, cx),
            SettingsTab::Associations => self.associations_page(cx),
            SettingsTab::Shortcuts => self.shortcuts_page(),
            SettingsTab::About => self.about_page(),
        };

        Some(
            div()
                .id("settings")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui_kit::rgba(0x000000b8))
                // 只拦住鼠标按下，让同位置的视频画面收不到这次点击。
                //
                // 刻意不在遮罩层上挂 `on_click` 去关窗口：GPUI 里祖先元素的 click
                // 监听先于里面的选项执行，一旦在这里关窗口，点选项就再也不生效了。
                // 收起交给右上角的 X 和 Esc。
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_this, _: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                    }),
                )
                .child(
                    div()
                        .id("settings-card")
                        .w(px(720.0))
                        .h(px(480.0))
                        // 窗口被拖小之后卡片不能顶出画面：超出的部分靠比例夹回去，
                        // 右侧内容自己滚。
                        .max_w(gpui_kit::relative(0.94))
                        .max_h(gpui_kit::relative(0.92))
                        .rounded(px(14.0))
                        .bg(surface())
                        .border_1()
                        .border_color(gpui_kit::rgb(LINE))
                        .shadow_lg()
                        .overflow_hidden()
                        .flex()
                        .flex_col()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_this, _: &MouseDownEvent, _window, cx| {
                                cx.stop_propagation();
                            }),
                        )
                        .child(self.render_settings_header(cx))
                        .child(div().h(px(1.0)).w_full().bg(gpui_kit::rgb(LINE)))
                        .child(
                            div()
                                .flex_1()
                                .min_h(px(0.0))
                                .flex()
                                // 左列固定宽，右侧内容自己滚：标签列跟着卡片走，
                                // 不跟着内容滚到看不见的地方。
                                .child(self.render_settings_tabs(cx))
                                .child(div().w(px(1.0)).h_full().bg(gpui_kit::rgb(LINE)))
                                .child(
                                    div()
                                        .id("settings-body")
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .p(px(20.0))
                                        .overflow_y_scroll()
                                        .child(body),
                                ),
                        ),
                ),
        )
    }

    fn render_settings_header(&self, cx: &mut Context<Self>) -> AnyElement {
        // 「关于」是从帮助菜单点进来的，顶着「更多设置」的标题会让人以为走错了地方。
        let (header_icon, header_title) = match self.settings_tab {
            SettingsTab::About => (IconName::Info, "关于 ngy-video-player"),
            _ => (IconName::Settings, "更多设置"),
        };

        div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(46.0))
            .px(px(16.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(icon(header_icon, 16.0, text_muted()))
                    .child(
                        div()
                            .text_size(px(14.0))
                            .text_color(text())
                            .child(header_title),
                    ),
            )
            .child(icon_button(
                "settings-close",
                IconName::X,
                false,
                false,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.set_settings_visible(false);
                    cx.notify();
                }),
            ))
            .into_any_element()
    }

    fn render_settings_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .w(px(150.0))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .p(px(10.0))
            .children(
                SettingsTab::ALL
                    .into_iter()
                    .map(|tab| self.render_settings_tab(tab, cx)),
            )
            // 恢复默认贴在左列底部：它作用于整扇窗口，放进某一页里会被当成那一页的开关。
            .child(div().flex_1())
            .child(settings_button(
                "settings-restore",
                "恢复默认设置",
                true,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.restore_default_settings(cx);
                }),
            ))
            .into_any_element()
    }

    fn render_settings_tab(&self, tab: SettingsTab, cx: &mut Context<Self>) -> AnyElement {
        let active = self.settings_tab == tab;
        div()
            .id(gpui_kit::ElementId::Name(
                format!("settings-tab-{}", tab.label()).into(),
            ))
            .flex()
            .items_center()
            .gap(px(9.0))
            .h(px(36.0))
            .flex_none()
            .px(px(10.0))
            .rounded(px(8.0))
            .text_size(px(13.0))
            .cursor(CursorStyle::PointingHand)
            .text_color(if active { text() } else { text_muted() })
            .when(active, |this| this.bg(ui::surface_active()))
            .when(!active, |this| {
                this.hover(|style| style.bg(ui::surface_hover()))
            })
            .child(icon(
                tab.icon(),
                15.0,
                if active { ui::accent() } else { text_muted() },
            ))
            .child(tab.label())
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                cx.stop_propagation();
                this.settings_tab = tab;
                cx.notify();
            }))
            .into_any_element()
    }
}
