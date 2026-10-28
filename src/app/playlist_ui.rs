//! 播放列表面板：标签页、列表与行渲染。

use super::*;

impl PlayerApp {
    /// 面板顶部的页签条：多份列表并排，`+` 新建，`×` 关闭（只剩一页时不显示）。
    pub(super) fn render_playlist_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.playlists.active();
        let closable = self.playlists.len() > 1;

        div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .px(px(8.0))
            .pb(px(6.0))
            .child(
                div()
                    .id("playlist-tabs")
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_x_scroll()
                    .children(
                        self.playlists
                            .tabs()
                            .iter()
                            .enumerate()
                            .map(|(index, tab)| {
                                let group = format!("playlist-tab-{index}");
                                let is_active = index == active;
                                div()
                                    .id(gpui_kit::ElementId::Name(
                                        format!("playlist-tab-{index}").into(),
                                    ))
                                    .group(group.clone())
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .h(px(24.0))
                                    .px(px(8.0))
                                    .rounded(px(6.0))
                                    .text_size(px(12.0))
                                    .cursor(CursorStyle::PointingHand)
                                    .text_color(if is_active {
                                        ui::accent()
                                    } else {
                                        text_muted()
                                    })
                                    .when(is_active, |this| this.bg(gpui_kit::rgb(0x232330)))
                                    .when(!is_active, |this| {
                                        this.hover(|style| style.bg(ui::surface_hover()))
                                    })
                                    // 切页不打断播放，所以「正在出声的是哪一页」得单独标出来。
                                    .when(self.playing_tab == Some(index), |this| {
                                        this.child(icon(IconName::Play, 8.0, ui::accent()))
                                    })
                                    .child(
                                        div()
                                            .max_w(px(140.0))
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(tab.name.clone()),
                                    )
                                    .when(closable, |this| {
                                        this.child(
                                            div()
                                                .id(gpui_kit::ElementId::Name(
                                                    format!("playlist-tab-close-{index}").into(),
                                                ))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .size(px(14.0))
                                                .rounded(px(4.0))
                                                // 和行尾的删除按钮一样：指到这一页才露出来。
                                                .opacity(0.0)
                                                .group_hover(group, |style| style.opacity(1.0))
                                                .hover(|style| style.bg(ui::surface_active()))
                                                .child(icon(IconName::X, 10.0, text_faint()))
                                                .on_click(cx.listener(
                                                    move |this, _: &ClickEvent, _window, cx| {
                                                        // 不截断的话这一下还会落到页签的「切换」上。
                                                        cx.stop_propagation();
                                                        this.close_tab(index, cx);
                                                    },
                                                )),
                                        )
                                    })
                                    .on_click(cx.listener(
                                        move |this, _: &ClickEvent, _window, cx| {
                                            this.select_tab(index, cx);
                                        },
                                    ))
                            }),
                    ),
            )
            .child(
                div()
                    .id("playlist-add-tab")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(22.0))
                    .rounded(px(6.0))
                    .text_size(px(14.0))
                    .text_color(text_muted())
                    .cursor(CursorStyle::PointingHand)
                    .hover(|style| style.bg(ui::surface_hover()).text_color(text()))
                    .child("+")
                    .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| this.add_tab(cx))),
            )
    }

    pub(super) fn render_playlist(
        &self,
        top: Pixels,
        width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let current = self.playlist().index();
        let len = self.playlist().len();
        let selected = self.selection.len();
        let multi_hint = if cfg!(target_os = "macos") {
            "⌘ 点击多选 · Shift 连选 · Delete 删除"
        } else {
            "Ctrl 点击多选 · Shift 连选 · Delete 删除"
        };

        div()
            .id("playlist")
            .absolute()
            .top(top)
            .right_0()
            .bottom_0()
            .w(px(width))
            .flex()
            .flex_col()
            .bg(gpui_kit::rgba(0x131317e6))
            .border_l_1()
            .border_color(gpui_kit::rgb(0x26262e))
            // 面板上的点击（选条目、点关闭）不该漏到画面去。
            .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                cx.stop_propagation();
                if this.menu_bar.is_some() {
                    this.menu_bar = None;
                    cx.notify();
                }
            }))
            // 跨在面板左边界上的拖动条：往左拖更宽，往右拖更窄。
            .child(
                div()
                    .id("playlist-resizer")
                    .absolute()
                    .left(px(-PLAYLIST_RESIZER_WIDTH / 2.0))
                    .top_0()
                    .bottom_0()
                    .w(px(PLAYLIST_RESIZER_WIDTH))
                    .cursor(CursorStyle::ResizeLeftRight)
                    .when(self.playlist_dragging, |this| this.bg(ui::accent()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.playlist_dragging = true;
                            this.drag_playlist_width(event.position.x, window);
                            cx.notify();
                        }),
                    ),
            )
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
                            .text_size(px(13.0))
                            .text_color(text_muted())
                            .child(format!("播放列表 · {len}")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.0))
                            // 用文字而不是垃圾桶图标：标题栏的图标离「关闭」太近，
                            // 而且和行尾的「移除这一项」撞脸，一不留神就清空了整张表。
                            .when(len > 0, |this| {
                                this.child(
                                    div()
                                        .id("playlist-clear")
                                        .px(px(6.0))
                                        .py(px(3.0))
                                        .rounded(px(6.0))
                                        .text_size(px(12.0))
                                        .text_color(text_muted())
                                        .cursor(CursorStyle::PointingHand)
                                        .hover(|style| {
                                            style.bg(ui::surface_hover()).text_color(ui::danger())
                                        })
                                        .child("清空")
                                        .on_click(cx.listener(
                                            |this, _: &ClickEvent, _window, cx| {
                                                cx.stop_propagation();
                                                this.clear_playlist(cx);
                                            },
                                        )),
                                )
                            })
                            .child(icon_button(
                                "playlist-close",
                                IconName::X,
                                false,
                                false,
                                cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    this.set_playlist_visible(false);
                                    cx.notify();
                                }),
                            )),
                    ),
            )
            .child(self.render_playlist_tabs(cx))
            // 有选择时才出现的一小条：把「删多少、删哪些」摆在手上。
            .when(selected > 0, |this| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px(px(14.0))
                        .pb(px(8.0))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(ui::accent())
                                .child(format!("已选 {selected} 项")),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .id("playlist-remove-selected")
                                        .px(px(6.0))
                                        .py(px(3.0))
                                        .rounded(px(6.0))
                                        .text_size(px(12.0))
                                        .text_color(ui::danger())
                                        .cursor(CursorStyle::PointingHand)
                                        .hover(|style| style.bg(ui::surface_hover()))
                                        .child("删除所选")
                                        .on_click(cx.listener(
                                            |this, _: &ClickEvent, _window, cx| {
                                                cx.stop_propagation();
                                                this.remove_selection(cx);
                                            },
                                        )),
                                )
                                .child(
                                    div()
                                        .id("playlist-clear-selection")
                                        .px(px(6.0))
                                        .py(px(3.0))
                                        .rounded(px(6.0))
                                        .text_size(px(12.0))
                                        .text_color(text_muted())
                                        .cursor(CursorStyle::PointingHand)
                                        .hover(|style| style.bg(ui::surface_hover()))
                                        .child("取消")
                                        .on_click(cx.listener(
                                            |this, _: &ClickEvent, _window, cx| {
                                                cx.stop_propagation();
                                                this.selection.clear();
                                                this.selection_anchor = None;
                                                cx.notify();
                                            },
                                        )),
                                ),
                        ),
                )
            })
            .when(len == 0, |this| {
                this.child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(8.0))
                        .child(icon(IconName::Film, 28.0, text_faint()))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(text_faint())
                                .child("列表为空"),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(text_faint())
                                .child("拖入文件，或按 O 打开"),
                        ),
                )
            })
            .when(len > 0, |this| {
                this.child(
                    div()
                        .id("playlist-scroll")
                        .flex_1()
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .children(self.playlist().items().iter().enumerate().map(
                            |(index, path)| self.render_playlist_row(index, path, current, cx),
                        )),
                )
                .child(
                    div()
                        .px(px(14.0))
                        .py(px(8.0))
                        .text_size(px(11.0))
                        .text_color(text_faint())
                        .child(multi_hint),
                )
            })
    }

    pub(super) fn render_playlist_row(
        &self,
        index: usize,
        path: &std::path::Path,
        current: Option<usize>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let name = path
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        let is_current = current == Some(index);
        let is_selected = self.selection.contains(&index);
        let group = format!("playlist-row-{index}");
        let row_path = path.to_path_buf();

        div()
            .id(gpui_kit::ElementId::Name(format!("item-{index}").into()))
            // 行尾的删除按钮只在这一行被指向时出现，靠 group hover 的样式下发；
            // 不用界面状态另存一个 hovered 索引，省掉每次进出都整窗重绘。
            .group(group.clone())
            .flex()
            .items_center()
            .gap(px(8.0))
            .pl(px(10.0))
            .pr(px(6.0))
            .py(px(8.0))
            .when(is_current, |this| this.bg(gpui_kit::rgb(0x1f2a3a)))
            .when(is_selected && !is_current, |this| {
                this.bg(gpui_kit::rgba(0x5aa9ff1f))
            })
            .when(!is_current, |this| {
                this.hover(|style| style.bg(ui::surface_hover()))
            })
            .cursor(CursorStyle::PointingHand)
            .child(
                // 勾选标记占固定一格，选中与未选中之间文字不会左右跳。
                div()
                    .w(px(14.0))
                    .flex()
                    .justify_center()
                    .when(is_selected, |this| {
                        this.child(icon(IconName::Check, 12.0, ui::accent()))
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(if is_current { ui::accent() } else { text() })
                            .child(ui::ellipsis(name)),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text_faint())
                            .child(formats::kind_label(path).to_string()),
                    ),
            )
            .child(
                div()
                    .id(gpui_kit::ElementId::Name(
                        format!("item-remove-{index}").into(),
                    ))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(24.0))
                    .rounded(px(6.0))
                    .cursor(CursorStyle::PointingHand)
                    // 占位与常显时一致，鼠标移上去才淡入，文字不会跟着左右跳。
                    .opacity(0.0)
                    .group_hover(group, |style| style.opacity(1.0))
                    .hover(|style| style.bg(ui::surface_hover()))
                    .child(icon(IconName::Trash, 14.0, text_faint()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        // 不截断的话，同一次点击还会落到整行的「换歌」上。
                        cx.stop_propagation();
                        this.remove_items(&[index], cx);
                    })),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
                this.select_item(index, event.modifiers(), cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    // 右键只弹菜单：不选中、不换歌，也不让点击漏到画面上。
                    cx.stop_propagation();
                    this.open_item_menu(index, row_path.clone(), event.position, window, cx);
                }),
            )
            .into_any_element()
    }
}
