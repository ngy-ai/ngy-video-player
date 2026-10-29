//! 播放列表排序：排序按钮、拖动重排、顺序历史。
//!
//! 排序本身是纯数据操作，放在 `playlist::sort` 里（好单测）；这里只负责界面：
//! 面板顶部那颗按钮、拖动的落点，以及改完之后落盘。菜单的浮层由
//! `playlist_menu` 统一画，排序只是其中的一组行。

use super::playlist_menu::PanelMenu;
use super::*;
use crate::playlist::SortMode;

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

impl PlayerApp {
    /// 面板标题栏上的「排序」按钮。
    pub(super) fn render_sort_button(&self, cx: &mut Context<Self>) -> AnyElement {
        icon_button_with_tip(
            "playlist-sort",
            IconName::ArrowUpDown,
            self.panel_menu == Some(PanelMenu::Sort),
            false,
            Some("排序与恢复顺序"),
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.toggle_panel_menu(PanelMenu::Sort, cx);
            }),
        )
    }

    /// 按某种方式排一次。列表是空的、或者顺序本来就这样时只给一句提示。
    pub(super) fn apply_sort(&mut self, mode: SortMode, cx: &mut Context<Self>) {
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
