//! 播放列表面板：标签页、列表与行渲染。

use super::playlist_sort::{DragGhost, PlaylistDrag};
use super::*;
use gpui_kit::DragMoveEvent;

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
                                    // 右键：重命名 / 删除这一页。左键会冒泡到面板的
                                    // 「点空白处收菜单」，所以右键必须先截断。
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(
                                            move |this, event: &MouseDownEvent, window, cx| {
                                                cx.stop_propagation();
                                                this.open_tab_menu(
                                                    index,
                                                    event.position,
                                                    window,
                                                    cx,
                                                );
                                            },
                                        ),
                                    )
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
            // 「全部列表」摆在 `+` 旁边：页签一多就横向滚出视野，
            // 滚出去的那些只能从这里找回。
            .child(self.render_lists_button(cx))
    }

    pub(super) fn render_playlist(
        &self,
        top: Pixels,
        width: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let empty = self.playlist().is_empty();
        let count = self.playlist().file_count();
        let selected = self.selection.len();
        let multi_hint = if cfg!(target_os = "macos") {
            "⌘ 点击多选 · Shift 连选 · Delete 删除 · 拖动排序、拖进文件夹"
        } else {
            "Ctrl 点击多选 · Shift 连选 · Delete 删除 · 拖动排序、拖进文件夹"
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
                if this.menu_bar.is_some() || this.panel_menu.is_some() || this.tab_menu.is_some() {
                    this.menu_bar = None;
                    this.panel_menu = None;
                    this.tab_menu = None;
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
                        // 标题吃掉剩余宽度并自行省略：面板拖窄时先让标题变短，
                        // 而不是把右边的按钮挤出面板。
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(13.0))
                            .text_color(text_muted())
                            .child(format!("播放列表 · {count}")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(2.0))
                            .child(self.render_add_button(cx))
                            // 排序按钮放在「清空」左边：它比清空常用，但两颗按钮
                            // 都贴着右上角，删表那一下不该是离手指最近的那个。
                            // 只在真有文件夹时出现：列表全是文件时它没有任何作用，
                            // 白白占掉标题栏的宽度。
                            .when(!empty && self.playlist().has_dir(), |this| {
                                this.child(self.render_fold_button(cx))
                            })
                            .when(!empty, |this| this.child(self.render_sort_button(cx)))
                            // 用文字而不是垃圾桶图标：标题栏的图标离「关闭」太近，
                            // 而且和行尾的「移除这一项」撞脸，一不留神就清空了整张表。
                            .when(!empty, |this| {
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
            .when(empty, |this| {
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
                                .child("点标题栏的添加按钮，或者拖入文件"),
                        ),
                )
            })
            .when(!empty, |this| {
                this.child(
                    div()
                        .id("playlist-scroll")
                        .flex_1()
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        // 收起文件夹时它后面那棵子树整段跳过，所以这里画的不是
                        // 数组的每一项，而是「界面上真正存在的行」。
                        .children(
                            self.playlist()
                                .visible_indices()
                                .into_iter()
                                .map(|index| self.render_playlist_row(index, cx)),
                        ),
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
            // 菜单必须是最后一个子节点：GPUI 按树的顺序绘制，排在行后面才盖得住它们。
            .children(self.render_panel_menu(cx))
    }

    /// 标题栏的「收起 / 展开所有文件夹」按钮。
    ///
    /// 一个按钮两态：已经全收起了就展开，否则收起。两个按钮挤进标题栏反而
    /// 分不清该点哪个，而且这颗按钮只在有文件夹时出现，本来就不常用。
    pub(super) fn render_fold_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let folded = self.playlist().all_collapsed();
        icon_button_with_tip(
            "playlist-fold",
            if folded {
                IconName::ChevronsUpDown
            } else {
                IconName::ChevronsDownUp
            },
            false,
            // 没有文件夹（比如只加了几个文件）时点不动。
            !self.playlist().has_dir(),
            Some(if folded {
                "展开所有文件夹"
            } else {
                "收起所有文件夹"
            }),
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.toggle_fold_all(cx);
            }),
        )
    }

    /// 一行：一个文件，或者一个可以折叠的文件夹。
    pub(super) fn render_playlist_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let playlist = self.playlist();
        let Some(item) = playlist.items().get(index) else {
            return div().into_any_element();
        };
        let path = item.path().to_path_buf();
        let depth = item.depth();
        let is_dir = item.is_dir();
        let folded = item.collapsed();
        let name = path
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        let current = playlist.index();
        let is_current = !is_dir && current == Some(index);
        // 正在播的那一项藏在收起的文件夹里时，把标记挪到文件夹行上：
        // 否则列表上根本找不到「现在放的是哪个」。
        let holds_current = is_dir
            && folded
            && current.is_some_and(|playing| playlist.is_descendant(playing, index));
        let subtitle = if is_dir {
            let files = playlist.dir_file_count(index);
            if holds_current {
                format!("播放中 · {files} 项")
            } else {
                format!("{files} 项")
            }
        } else {
            formats::kind_label(&path).to_string()
        };

        let is_selected = self.selection.contains(&index);
        let group = format!("playlist-row-{index}");
        let row_path = path.clone();
        // 拖动落点：横线表示插到这一行前面/后面，整行高亮表示放进这个文件夹。
        //
        // 只在真的拖着一个列表行时才画。拖动被半路取消时（松手位置在列表行
        // 之外）落点没有机会被清掉，靠这个条件保证它不会一直留在界面上。
        let dragging = cx.has_active_drag();
        let line_above = dragging && self.drop_spot == Some(DropSpot::Before(index));
        let line_below = dragging && self.drop_spot == Some(DropSpot::After(index));
        let drop_into = dragging && self.drop_spot == Some(DropSpot::Into(index));
        let drag_name: SharedString = name.clone().into();

        div()
            .id(gpui_kit::ElementId::Name(format!("item-{index}").into()))
            // 行尾的删除按钮只在这一行被指向时出现，靠 group hover 的样式下发；
            // 不用界面状态另存一个 hovered 索引，省掉每次进出都整窗重绘。
            .group(group.clone())
            .relative()
            .flex()
            .items_center()
            .gap(px(8.0))
            // 缩进就是层级本身：子项比父项多缩进一格。
            .pl(px(10.0 + f32::from(depth) * PLAYLIST_INDENT))
            .pr(px(6.0))
            .py(px(7.0))
            .when(line_above, |this| this.child(drop_line(false)))
            .when(line_below, |this| this.child(drop_line(true)))
            .when(drop_into, |this| {
                this.bg(gpui_kit::rgba(0x5aa9ff26))
                    .border_1()
                    .border_color(ui::accent())
            })
            .when(is_current || holds_current, |this| {
                this.bg(gpui_kit::rgb(0x1f2a3a))
            })
            .when(is_selected && !is_current && !holds_current, |this| {
                this.bg(gpui_kit::rgba(0x5aa9ff1f))
            })
            .when(!is_current && !holds_current, |this| {
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
            .when(is_dir, |this| {
                // 折叠三角 + 文件夹图标占的宽度和文件行对齐，展开收起时文字不跳。
                this.child(div().w(px(12.0)).flex().justify_center().child(icon(
                    if folded {
                        IconName::ChevronRight
                    } else {
                        IconName::ChevronDown
                    },
                    12.0,
                    text_muted(),
                )))
                .child(icon(
                    if folded {
                        IconName::Folder
                    } else {
                        IconName::FolderOpen
                    },
                    14.0,
                    if holds_current {
                        ui::accent()
                    } else {
                        text_muted()
                    },
                ))
            })
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(px(13.0))
                            .text_color(if is_current || holds_current {
                                ui::accent()
                            } else if is_dir {
                                text_muted()
                            } else {
                                text()
                            })
                            .child(ui::ellipsis(name)),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text_faint())
                            .child(subtitle),
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
                    this.open_item_menu(
                        index,
                        row_path.clone(),
                        is_dir,
                        event.position,
                        window,
                        cx,
                    );
                }),
            )
            // 整行可以拖：拖到别的行前面/后面是改顺序，落到文件夹中间是放进去。
            // 拖动本身由 GPUI 的拖放机制接管，拖动过的这一次点击不会再触发「换歌」。
            .on_drag(
                PlaylistDrag {
                    from: index,
                    name: drag_name,
                    dir: is_dir,
                },
                |drag, _, _, cx| {
                    cx.new(|_| DragGhost {
                        name: drag.name.clone(),
                        dir: drag.dir,
                    })
                },
            )
            .on_drag_move(cx.listener(
                move |this, event: &DragMoveEvent<PlaylistDrag>, _window, cx| {
                    // 这个回调是全局的：拖动期间每一行都会收到同一次移动，
                    // 得自己筛出指针真正压着的那一行。
                    let position = event.event.position;
                    if !event.bounds.contains(&position) {
                        return;
                    }
                    // 文件行：上半截插到它前面，下半截插到它后面。
                    // 文件夹行：上下两截同样是改顺序，中间那一截是「放进去」。
                    let top = f32::from(event.bounds.origin.y);
                    let height = f32::from(event.bounds.size.height).max(1.0);
                    let ratio = (f32::from(position.y) - top) / height;
                    let spot = if is_dir {
                        if ratio < 0.3 {
                            DropSpot::Before(index)
                        } else if ratio > 0.7 {
                            DropSpot::After(index)
                        } else {
                            DropSpot::Into(index)
                        }
                    } else if ratio < 0.5 {
                        DropSpot::Before(index)
                    } else {
                        DropSpot::After(index)
                    };
                    this.set_drop_spot(Some(spot), cx);
                },
            ))
            .on_drop(cx.listener(move |this, drag: &PlaylistDrag, _window, cx| {
                this.drop_on_row(drag.from, index, cx);
            }))
            .into_any_element()
    }
}

/// 拖动落点的那条横线。`below` 画在行底，表示「插到这一行后面」。
fn drop_line(below: bool) -> AnyElement {
    div()
        .absolute()
        .left_0()
        .right_0()
        .h(px(2.0))
        .bg(ui::accent())
        .when(below, |this| this.bottom_0())
        .when(!below, |this| this.top(px(-1.0)))
        .into_any_element()
}
