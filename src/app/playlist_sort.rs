//! 播放列表排序：排序按钮、拖动重排、顺序历史。
//!
//! 排序本身是纯数据操作，放在 `playlist::sort` 里（好单测）；这里只负责界面：
//! 面板顶部那颗按钮、拖动的落点，以及改完之后落盘。菜单的浮层由
//! `playlist_menu` 统一画，排序只是其中的一组行。

use super::playlist_menu::PanelMenu;
use super::*;
use crate::playlist::{DropSpot, SortMode};

/// 拖动中的一行。GPUI 的拖放按值的类型分发，用专用类型就不会和「从资源管理器
/// 拖文件进来」混在一起。
pub(super) struct PlaylistDrag {
    pub(super) from: usize,
    pub(super) name: SharedString,
    /// 拖的是文件夹。拖动预览上标出来，拖一整个文件夹时心里有数。
    pub(super) dir: bool,
}

/// 跟着鼠标走的拖动预览：一颗只显示名字的胶囊。
pub(super) struct DragGhost {
    pub(super) name: SharedString,
    pub(super) dir: bool,
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
            .flex()
            .items_center()
            .gap(px(6.0))
            .overflow_hidden()
            .text_size(px(12.0))
            .text_color(text())
            .when(self.dir, |this| {
                this.child(icon(IconName::Folder, 12.0, ui::accent()))
            })
            .child(
                div()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(self.name.clone()),
            )
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
            Some("排序 · 顺序历史 · 文件夹折叠"),
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
        self.drop_spot = None;
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
        self.drop_spot = None;
        self.playlists.save();
        self.flash(if restored {
            "已恢复到选定的顺序".to_string()
        } else {
            "顺序已经就是这一版".to_string()
        });
        self.poke();
        cx.notify();
    }

    /// 拖动落点。相同落点直接返回，避免整窗重绘。
    pub(super) fn set_drop_spot(&mut self, spot: Option<DropSpot>, cx: &mut Context<Self>) {
        if self.drop_spot == spot {
            return;
        }
        self.drop_spot = spot;
        cx.notify();
    }

    /// 松手：只在落点确实落在这一行上时才动列表。
    ///
    /// 拖动被半路取消（松手时已经在面板外）时落点会留着上一轮的值，
    /// 「落点必须锚定当前行」这条把过期数据挡掉了。
    pub(super) fn drop_on_row(&mut self, from: usize, row: usize, cx: &mut Context<Self>) {
        let spot = self.drop_spot.filter(|spot| spot.anchor() == row);
        self.drop_spot = None;
        let Some(spot) = spot else {
            cx.notify();
            return;
        };
        self.reorder_item(from, spot, cx);
    }

    /// 手工拖动：把第 `from` 项（连它的子树）搬到 `spot`。
    pub(super) fn reorder_item(&mut self, from: usize, spot: DropSpot, cx: &mut Context<Self>) {
        if !self.playlist_mut().move_to(from, spot) {
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
