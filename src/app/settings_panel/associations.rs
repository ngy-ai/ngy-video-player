//! 设置窗口 ·「文件关联」页：注册、撤销与启动时维护。
//!
//! 原来这些入口全挤在菜单栏的「设置」菜单里：注册和撤销是破坏性操作，和普通勾选项
//! 排在同一条竖线上很容易点错。搬进设置窗口之后它们有了自己的分组和间距。

use super::super::*;

impl PlayerApp {
    pub(super) fn associations_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let supported = platform::associations::supported();
        let status = &self.association_status;
        let registered = status.is_registered();

        let mut default_rows = vec![settings_button(
            "settings-association-set-default",
            "设为默认视频播放器",
            supported,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.set_default_player();
                cx.notify();
            }),
        )];
        // 只有 Windows 需要用户去系统设置里再点一次；另两个平台注册即生效。
        if cfg!(windows) {
            default_rows.push(settings_button(
                "settings-association-open-apps",
                "打开系统默认应用设置",
                supported,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.open_default_apps();
                    cx.notify();
                }),
            ));
        }

        let maintain_row = picture_option(
            "settings-association-maintain",
            "启动时自动维护文件关联",
            self.settings.maintain_associations,
            true,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.toggle_maintain_associations();
                cx.notify();
            }),
        );

        div()
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(option_group(
                "当前状态",
                vec![
                    div()
                        .px(px(8.0))
                        .py(px(4.0))
                        .text_size(px(13.0))
                        .text_color(text())
                        .child(association_status_line(status, supported))
                        .into_any_element(),
                ],
            ))
            .child(option_group(
                "默认播放器",
                vec![
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(8.0))
                        .children(default_rows)
                        .into_any_element(),
                ],
            ))
            .child(option_group("启动时", vec![maintain_row]))
            .child(option_group(
                "撤销",
                vec![
                    div()
                        .flex()
                        .child(settings_button(
                            "settings-association-remove",
                            "移除本程序的文件关联",
                            registered && supported,
                            cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.remove_associations(window, cx);
                            }),
                        ))
                        .into_any_element(),
                ],
            ))
            .child(settings_note(
                "Windows 上登记之后，还要在系统「默认应用」里点一次「设为默认值」；\
                 撤销只会删掉本程序登记的项，已经选定的默认播放器不受影响。",
            ))
            .into_any_element()
    }
}
