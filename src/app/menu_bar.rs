//! 菜单栏渲染：下拉、条目与勾选状态。

use super::*;

impl PlayerApp {
    pub(super) fn render_menu_bar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id("menu-bar")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(MENUBAR_HEIGHT))
            .flex()
            .items_center()
            .px(px(8.0))
            .bg(surface())
            .border_b_1()
            .border_color(gpui_kit::rgb(0x26262e))
            .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                cx.stop_propagation();
            }))
            .children(
                BarMenu::ALL
                    .iter()
                    .map(|menu| self.render_bar_menu(*menu, window, cx).into_any_element()),
            )
    }

    pub(super) fn render_bar_menu(
        &self,
        menu: BarMenu,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open = self.menu_bar == Some(menu);
        div()
            .relative()
            .child(
                div()
                    .id(gpui_kit::ElementId::Name(
                        format!("bar-menu-{menu:?}").into(),
                    ))
                    .h(px(26.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .rounded(px(6.0))
                    .text_size(px(13.0))
                    .cursor(CursorStyle::PointingHand)
                    .text_color(if open { ui::accent() } else { text_muted() })
                    .when(open, |this| this.bg(ui::surface_hover()))
                    .when(!open, |this| {
                        this.hover(|style| style.bg(ui::surface_hover()))
                    })
                    .child(menu.label())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.menu_bar = (this.menu_bar != Some(menu)).then_some(menu);
                        if this.menu_bar.is_some() {
                            this.menu = None;
                        }
                        if this.menu_bar == Some(BarMenu::Settings) {
                            this.refresh_association_status();
                        }
                        this.poke();
                        cx.notify();
                    }))
                    // 已经展开一个菜单时，移过其它标题直接切换，符合桌面菜单习惯。
                    .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                        if *hovered && this.menu_bar.is_some() && this.menu_bar != Some(menu) {
                            this.menu_bar = Some(menu);
                            if menu == BarMenu::Settings {
                                this.refresh_association_status();
                            }
                            this.poke();
                            cx.notify();
                        }
                    })),
            )
            .when(open, |this| {
                this.child(self.render_bar_dropdown(menu, window, cx))
            })
    }

    pub(super) fn render_bar_dropdown(
        &self,
        menu: BarMenu,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let available = f32::from(window.viewport_size().height) - MENUBAR_HEIGHT - 16.0;
        div()
            .id(gpui_kit::ElementId::Name(
                format!("bar-dropdown-{menu:?}").into(),
            ))
            .absolute()
            .left(px(-4.0))
            .top(px(28.0))
            .w(px(250.0))
            .max_h(px(available.clamp(160.0, 420.0)))
            .overflow_y_scroll()
            .py(px(5.0))
            .rounded(px(10.0))
            .bg(surface())
            .border_1()
            .border_color(gpui_kit::rgb(0x2c2c34))
            .shadow_lg()
            .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                cx.stop_propagation();
            }))
            .children(self.bar_entries(menu, window).into_iter().enumerate().map(
                |(index, item)| {
                    match item {
                        BarItem::Separator => div()
                            .h(px(1.0))
                            .mx(px(8.0))
                            .my(px(4.0))
                            .bg(gpui_kit::rgb(0x2c2c34))
                            .into_any_element(),
                        BarItem::Entry(entry) => self
                            .render_bar_entry(menu, index, entry, cx)
                            .into_any_element(),
                    }
                },
            ))
    }

    pub(super) fn render_bar_entry(
        &self,
        menu: BarMenu,
        index: usize,
        entry: BarEntry,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let BarEntry {
            label,
            hint,
            active,
            enabled,
            action,
        } = entry;
        let action = action.filter(|_| enabled);
        let color = if !enabled {
            text_faint()
        } else if active {
            ui::accent()
        } else {
            text()
        };

        div()
            .id(gpui_kit::ElementId::Name(
                format!("bar-item-{menu:?}-{index}").into(),
            ))
            .flex()
            .items_center()
            .gap(px(8.0))
            .h(px(30.0))
            .mx(px(4.0))
            .px(px(10.0))
            .rounded(px(6.0))
            .text_size(px(13.0))
            .text_color(color)
            .when(enabled, |this| {
                this.cursor(CursorStyle::PointingHand)
                    .hover(|style| style.bg(ui::surface_hover()))
            })
            .child(
                div()
                    .w(px(14.0))
                    .flex()
                    .justify_center()
                    .when(active, |this| {
                        this.child(icon(IconName::Check, 14.0, ui::accent()))
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(label),
            )
            .when_some(hint, |this, hint| {
                this.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(text_faint())
                        .child(hint),
                )
            })
            .when_some(action, |this, action| {
                this.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.run_bar_action(action.clone(), window, cx);
                }))
            })
    }
}
