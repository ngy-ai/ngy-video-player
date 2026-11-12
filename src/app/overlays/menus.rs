//! 弹出菜单：列表条目菜单、页签菜单、页签重命名。

use crate::app::*;

impl PlayerApp {
    /// 行右键菜单：贴在鼠标位置，三项都是对「这一个文件」的操作。
    pub(in crate::app) fn render_item_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let menu = self.item_menu.as_ref()?;
        let index = menu.index;
        let is_dir = menu.dir;
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
                    if is_dir {
                        "在文件夹中定位"
                    } else {
                        "定位文件"
                    },
                    text(),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.reveal_item(reveal_path.clone(), cx);
                    }),
                ))
                // 文件夹没有格式可看，换成折叠开关——这是它最常用的操作。
                .child(if is_dir {
                    item_menu_entry(
                        "item-menu-toggle",
                        IconName::FolderOpen,
                        "展开 / 收起",
                        text(),
                        cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            cx.stop_propagation();
                            this.item_menu = None;
                            this.toggle_dir(index, cx);
                        }),
                    )
                } else {
                    item_menu_entry(
                        "item-menu-info",
                        IconName::Info,
                        "查看格式",
                        text(),
                        cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            cx.stop_propagation();
                            this.show_file_info(info_path.clone(), cx);
                        }),
                    )
                }),
        )
    }

    /// 页签右键菜单：重命名 / 删除这一页列表。
    pub(in crate::app) fn render_tab_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let menu = self.tab_menu.as_ref()?;
        let index = menu.index;
        // 默认列表不能改名、不能删；只剩一页时也不能删：右上角关闭按钮也是
        // 这个规矩，几条路别给出两种答案。
        let is_default = self.playlists.is_default(index);
        let closable = self.playlists.len() > 1 && !is_default;

        Some(
            div()
                .id("tab-menu")
                .absolute()
                .left(menu.position.x)
                .top(menu.position.y)
                .w(px(TAB_MENU_WIDTH))
                .py(px(6.0))
                .rounded(px(10.0))
                .bg(surface())
                .border_1()
                .border_color(gpui_kit::rgb(0x2c2c34))
                .shadow_lg()
                // 菜单上的点击统一吃掉：别漏到画面去切播放状态，也别让根节点的
                // 「点别处收菜单」在 mouse-down 阶段就把它拆掉。
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
                    "tab-menu-rename",
                    IconName::Pencil,
                    if is_default {
                        "默认列表，不可改名"
                    } else {
                        "重命名…"
                    },
                    if is_default { text_faint() } else { text() },
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        // 灰掉的那一行也留着点击：点了给一句原因，比毫无反应好。
                        this.begin_rename(index, cx);
                    }),
                ))
                .child(item_menu_entry(
                    "tab-menu-delete",
                    IconName::Trash,
                    if is_default {
                        "默认列表，不可删除"
                    } else if closable {
                        "删除列表"
                    } else {
                        "仅剩一页，无法删除"
                    },
                    if closable { ui::danger() } else { text_faint() },
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.tab_menu = None;
                        if closable {
                            this.close_tab(index, cx);
                        } else if is_default {
                            this.flash("「默认」列表不能删除");
                            cx.notify();
                        } else {
                            this.flash("只剩一页列表，不能删除");
                            cx.notify();
                        }
                    }),
                )),
        )
    }

    /// 重命名列表的输入框。模态：遮住画面，点别处不生效，键盘走 `on_rename_key`。
    pub(in crate::app) fn render_tab_rename(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let dialog = self.tab_rename.as_ref()?;
        let value = dialog.text.clone();
        let empty = value.trim().is_empty();

        Some(
            div()
                .id("tab-rename")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui_kit::rgba(0x000000b8))
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .child(
                    div()
                        .w(px(420.0))
                        // 窗口拖窄时面板不能顶出画面，否则「确定」会落到窗口外。
                        .max_w(gpui_kit::relative(0.92))
                        .p(px(18.0))
                        .rounded(px(14.0))
                        .bg(surface())
                        .border_1()
                        .border_color(gpui_kit::rgb(0x2c2c34))
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .child(
                            div()
                                .text_size(px(15.0))
                                .text_color(text())
                                .child("重命名列表"),
                        )
                        .child(
                            div()
                                .id("tab-rename-input")
                                .h(px(34.0))
                                .flex()
                                .items_center()
                                .px(px(10.0))
                                .rounded(px(8.0))
                                .bg(ui::surface_active())
                                .border_1()
                                .border_color(ui::accent())
                                .cursor(CursorStyle::IBeam)
                                .overflow_hidden()
                                .when(!value.is_empty(), |this| {
                                    this.child(
                                        div()
                                            .min_w(px(0.0))
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .text_size(px(13.0))
                                            .text_color(text())
                                            .child(value.clone()),
                                    )
                                })
                                // 光标固定画在末尾：不做左右移动与选区，输入永远是追加/退格。
                                .child(div().ml(px(1.0)).w(px(1.0)).h(px(16.0)).bg(ui::accent())),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(if empty { ui::danger() } else { text_faint() })
                                .child(if empty {
                                    "列表名不能为空"
                                } else {
                                    "Enter 确认 · Esc 取消 · 首次输入会替换原名"
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap(px(6.0))
                                .child(dialog_button(
                                    "tab-rename-cancel",
                                    "取消",
                                    false,
                                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                                        cx.stop_propagation();
                                        this.tab_rename = None;
                                        cx.notify();
                                    }),
                                ))
                                .child(dialog_button(
                                    "tab-rename-ok",
                                    "确定",
                                    true,
                                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                                        cx.stop_propagation();
                                        this.finish_rename(cx);
                                    }),
                                )),
                        ),
                ),
        )
    }
}

/// 对话框底部的文字按钮。`primary` 用强调色填充，其余是次要操作。
fn dialog_button(
    id: &'static str,
    label: &'static str,
    primary: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .px(px(12.0))
        .py(px(6.0))
        .rounded(px(7.0))
        .text_size(px(12.0))
        .cursor(CursorStyle::PointingHand)
        .when(primary, |this| {
            this.bg(ui::accent()).text_color(gpui_kit::rgb(0x111114))
        })
        .when(!primary, |this| {
            this.text_color(text_muted())
                .hover(|style| style.bg(ui::surface_hover()))
        })
        .child(label)
        .on_click(on_click)
        .into_any_element()
}
