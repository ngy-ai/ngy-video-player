//! 播放列表排序：排序菜单、拖动重排、顺序历史。
//!
//! 排序本身是纯数据操作，放在 `playlist::sort` 里（好单测）；这里只负责界面：
//! 面板顶部那颗按钮、弹出菜单、拖动的落点，以及改完之后落盘。

use super::*;
use crate::playlist::SortMode;

/// 排序菜单离面板顶端的距离。标题行是 34 的按钮加 8+8 的内边距，正好 50，
/// 再让开 2 像素，菜单就像从「排序」按钮下面拉出来的一样。
const MENU_TOP: f32 = 52.0;

/// 拖动中的一行。GPUI 的拖放按值的类型分发，用专用类型就不会和「从资源管理器
/// 拖文件进来」混在一起。
pub(super) struct PlaylistDrag {
    pub(super) from: usize,
    pub(super) name: SharedString,
}

/// 跟着鼠标走的拖动预览：一颗只显示文件名的胶囊。
pub(super) struct DragGhost {
    pub(super) name: SharedString,
}

impl Render for DragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(240.0))
            .px(px(10.0))
            .py(px(6.0))
            .rounded(px(6.0))
            .bg(gpui_kit::rgba(0x232330f2))
            .border_1()
            .border_color(ui::accent())
            .overflow_hidden()
            .text_size(px(12.0))
            .text_color(text())
            .whitespace_nowrap()
            .text_ellipsis()
            .child(self.name.clone())
    }
}

/// 菜单里的一行。
enum SortRow {
    /// 分组标题，不可点。
    Title(&'static str),
    Divider,
    Entry {
        label: String,
        active: bool,
        /// `None` 是纯说明，灰着不可点。
        action: Option<SortAction>,
    },
}

#[derive(Clone, Copy)]
enum SortAction {
    Apply(SortMode),
    Restore(usize),
}

impl PlayerApp {
    /// 面板标题栏上的「排序」按钮。
    pub(super) fn render_sort_button(&self, cx: &mut Context<Self>) -> AnyElement {
        icon_button_with_tip(
            "playlist-sort",
            IconName::ArrowUpDown,
            self.sort_menu,
            false,
            Some("排序与恢复顺序"),
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.sort_menu = !this.sort_menu;
                this.poke();
                cx.notify();
            }),
        )
    }

    pub(super) fn render_sort_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        // 列表被清空后「排序」按钮就不在了，菜单也跟着收掉，免得留一张空壳挂在面板上。
        if !self.sort_menu || self.playlist().is_empty() {
            return None;
        }

        let mode = self.playlist().sort_mode();
        let mut rows = vec![SortRow::Title("排序方式")];
        for candidate in [SortMode::Name, SortMode::Modified, SortMode::Added] {
            rows.push(SortRow::Entry {
                label: candidate.menu_label().to_string(),
                active: mode == candidate,
                action: Some(SortAction::Apply(candidate)),
            });
        }
        if mode == SortMode::Manual {
            rows.push(SortRow::Entry {
                label: "当前：手工排序（拖动行调整）".to_string(),
                active: true,
                action: None,
            });
        }

        rows.push(SortRow::Divider);
        rows.push(SortRow::Title("恢复顺序（最近 3 版）"));
        let history = self.playlist().history();
        if history.is_empty() {
            rows.push(SortRow::Entry {
                label: "还没有可恢复的顺序".to_string(),
                active: false,
                action: None,
            });
        } else {
            for (index, version) in history.iter().enumerate() {
                rows.push(SortRow::Entry {
                    label: format!(
                        "第 {} 版 · {}（{} 项）",
                        index + 1,
                        version.mode().label(),
                        version.len()
                    ),
                    active: false,
                    action: Some(SortAction::Restore(index)),
                });
            }
        }

        Some(
            div()
                .id("playlist-sort-menu")
                .absolute()
                .top(px(MENU_TOP))
                .right(px(6.0))
                .w(px(216.0))
                .py(px(4.0))
                .rounded(px(10.0))
                .bg(surface())
                .border_1()
                .border_color(gpui_kit::rgb(0x2c2c34))
                .shadow_lg()
                // 菜单自己的点击不再往面板上冒，否则点一下就把菜单关了。
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .children(rows.into_iter().enumerate().map(|(index, row)| {
                    let id = gpui_kit::ElementId::Name(format!("sort-row-{index}").into());
                    match row {
                        SortRow::Title(text) => div()
                            .px(px(12.0))
                            .pt(px(6.0))
                            .pb(px(2.0))
                            .text_size(px(11.0))
                            .text_color(text_faint())
                            .child(text)
                            .into_any_element(),
                        SortRow::Divider => div()
                            .h(px(1.0))
                            .mx(px(8.0))
                            .my(px(4.0))
                            .bg(gpui_kit::rgb(0x2c2c34))
                            .into_any_element(),
                        SortRow::Entry {
                            label,
                            active,
                            action,
                        } => div()
                            .id(id)
                            .px(px(10.0))
                            .py(px(6.0))
                            .mx(px(4.0))
                            .rounded(px(6.0))
                            .text_size(px(12.0))
                            .text_color(if active {
                                ui::accent()
                            } else if action.is_some() {
                                text()
                            } else {
                                text_faint()
                            })
                            .when(action.is_some(), |this| {
                                this.cursor(CursorStyle::PointingHand)
                                    .hover(|style| style.bg(ui::surface_hover()))
                            })
                            .child(label)
                            .when_some(action, |this, action| {
                                this.on_click(cx.listener(
                                    move |this, _: &ClickEvent, _window, cx| {
                                        cx.stop_propagation();
                                        match action {
                                            SortAction::Apply(mode) => this.apply_sort(mode, cx),
                                            SortAction::Restore(index) => {
                                                this.restore_order(index, cx);
                                            }
                                        }
                                    },
                                ))
                            })
                            .into_any_element(),
                    }
                }))
                .into_any_element(),
        )
    }

    /// 按某种方式排一次。列表是空的、或者顺序本来就这样时只给一句提示。
    pub(super) fn apply_sort(&mut self, mode: SortMode, cx: &mut Context<Self>) {
        self.sort_menu = false;
        let changed = self.playlist_mut().sort_items(mode);
        self.selection.clear();
        self.selection_anchor = None;
        self.playlists.save();
        self.flash(if changed {
            format!("已{}排序", mode.label())
        } else {
            format!("{}：顺序没有变化", mode.label())
        });
        self.poke();
        cx.notify();
    }

    pub(super) fn restore_order(&mut self, index: usize, cx: &mut Context<Self>) {
        self.sort_menu = false;
        let restored = self.playlist_mut().restore_version(index);
        self.selection.clear();
        self.selection_anchor = None;
        self.playlists.save();
        self.flash(if restored {
            "已恢复到选定的顺序".to_string()
        } else {
            "顺序已经就是这一版".to_string()
        });
        self.poke();
        cx.notify();
    }

    /// 拖动落点：`Some(n)` 表示松手时插到原列表第 `n` 项之前。
    pub(super) fn set_sort_drop(&mut self, target: Option<usize>, cx: &mut Context<Self>) {
        if self.sort_drop == target {
            return;
        }
        self.sort_drop = target;
        cx.notify();
    }

    /// 松手：只在落点确实挨着这一行时才动列表。
    ///
    /// 拖动被半路取消（松手时已经在面板外）时 `sort_drop` 会留着上一轮的值，
    /// 「落点必须和当前行相邻」这条把过期数据挡掉了。
    pub(super) fn drop_on_row(&mut self, from: usize, row: usize, cx: &mut Context<Self>) {
        let target = self
            .sort_drop
            .filter(|target| *target == row || *target == row + 1);
        self.sort_drop = None;
        let Some(target) = target else {
            cx.notify();
            return;
        };
        self.reorder_item(from, target, cx);
    }

    /// 手工拖动重排：把第 `from` 项插到 `insert_at`（移动前的下标）之前。
    pub(super) fn reorder_item(&mut self, from: usize, insert_at: usize, cx: &mut Context<Self>) {
        if !self.playlist_mut().move_item(from, insert_at) {
            cx.notify();
            return;
        }
        self.selection.clear();
        self.selection_anchor = None;
        self.playlists.save();
        self.poke();
        cx.notify();
    }
}
