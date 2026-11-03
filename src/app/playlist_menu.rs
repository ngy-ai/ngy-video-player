//! 播放列表面板上的弹出菜单：添加（文件 / 文件夹）、排序、全部列表。
//!
//! 几个菜单共用一套行结构和同一个浮层，只是锚点不同，所以放在同一个模块里；
//! 菜单里真正干活的逻辑还留在各自的模块（添加在 `playlist_ops`，排序在
//! `playlist_sort`，切页在 `playlist_ops`）。

use super::*;
use crate::playlist::SortMode;

/// 标题栏上的菜单离面板顶端的距离：标题行是 34 的按钮加 8+8 的内边距，正好 50，
/// 再让开 2 像素，菜单就像从按钮下面拉出来的一样。
const HEADER_MENU_TOP: f32 = 52.0;
/// 「全部列表」挂在页签条下面：标题行 50 加页签条（24 高 + 6 下边距）。
const TAB_MENU_TOP: f32 = 80.0;

/// 面板上展开着的是哪个菜单。用 `Option` 而不是几个 bool：
/// 同一个位置只可能有一个浮层，状态里就不该能表示出「两个都开着」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PanelMenu {
    Add,
    Sort,
    /// 页签条上的「全部列表」：页签横向滚出视野时，这里是唯一的全量入口。
    Lists,
}

impl PanelMenu {
    fn top(self) -> f32 {
        match self {
            PanelMenu::Add | PanelMenu::Sort => HEADER_MENU_TOP,
            PanelMenu::Lists => TAB_MENU_TOP,
        }
    }
}

/// 菜单里的一行。
enum PanelRow {
    /// 分组标题，不可点。
    Title(&'static str),
    Divider,
    Entry {
        label: String,
        active: bool,
        /// `None` 是纯说明，灰着不可点。
        action: Option<PanelAction>,
    },
}

#[derive(Clone, Copy)]
enum PanelAction {
    AddFiles,
    AddFolder,
    ApplySort(SortMode),
    Restore(usize),
    SelectTab(usize),
    /// 一键收起 / 展开所有文件夹。
    FoldAll(bool),
}

impl PlayerApp {
    /// 「添加」按钮：列表空着的时候它是唯一还能用的那颗，所以永远显示。
    pub(super) fn render_add_button(&self, cx: &mut Context<Self>) -> AnyElement {
        icon_button_with_tip(
            "playlist-add",
            // 带文件的加号而不是光秃秃的 `+`：页签条上那个 `+` 是「新建一份
            // 列表」，两颗按钮长得一样会点错。
            IconName::FilePlus,
            self.panel_menu == Some(PanelMenu::Add),
            false,
            Some("添加文件或文件夹到当前列表"),
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.toggle_panel_menu(PanelMenu::Add, cx);
            }),
        )
    }

    pub(super) fn toggle_panel_menu(&mut self, menu: PanelMenu, cx: &mut Context<Self>) {
        self.panel_menu = (self.panel_menu != Some(menu)).then_some(menu);
        // 面板菜单和页签菜单是两层浮层，同时开着会互相盖住。
        self.tab_menu = None;
        self.poke();
        cx.notify();
    }

    /// 页签条上的「全部列表」按钮：页签多了会滚出视野，这里是全量入口。
    pub(super) fn render_lists_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let open = self.panel_menu == Some(PanelMenu::Lists);
        div()
            .id("playlist-all-tabs")
            .flex()
            .items_center()
            .justify_center()
            .size(px(22.0))
            .rounded(px(6.0))
            .cursor(CursorStyle::PointingHand)
            .when(open, |this| this.bg(ui::surface_active()))
            .hover(|style| style.bg(ui::surface_hover()))
            .child(icon(
                IconName::List,
                14.0,
                if open { ui::accent() } else { text_muted() },
            ))
            .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                // 不截断的话这一下还会落到面板的「点空白处收菜单」上，
                // 刚打开就又被关掉了。
                cx.stop_propagation();
                this.toggle_panel_menu(PanelMenu::Lists, cx);
            }))
            .into_any_element()
    }

    pub(super) fn render_panel_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.panel_menu?;
        // 列表被清空后「排序」按钮就不在了，菜单也跟着收掉，免得留一张空壳。
        if menu == PanelMenu::Sort && self.playlist().is_empty() {
            return None;
        }
        let rows = match menu {
            PanelMenu::Add => Self::add_rows(),
            PanelMenu::Sort => self.sort_rows(),
            PanelMenu::Lists => self.list_rows(),
        };

        Some(
            div()
                .id("playlist-panel-menu")
                .absolute()
                .top(px(menu.top()))
                .right(px(6.0))
                .w(px(216.0))
                .py(px(4.0))
                // 页签特别多时菜单会顶到窗口外面去，封个高度让它自己滚。
                .max_h(px(420.0))
                .overflow_y_scroll()
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
                    let id = gpui_kit::ElementId::Name(format!("panel-menu-{index}").into());
                    match row {
                        PanelRow::Title(text) => div()
                            .px(px(12.0))
                            .pt(px(6.0))
                            .pb(px(2.0))
                            .text_size(px(11.0))
                            .text_color(text_faint())
                            .child(text)
                            .into_any_element(),
                        PanelRow::Divider => div()
                            .h(px(1.0))
                            .mx(px(8.0))
                            .my(px(4.0))
                            .bg(gpui_kit::rgb(0x2c2c34))
                            .into_any_element(),
                        PanelRow::Entry {
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
                            // 列表名可能很长，菜单宽度是定死的，统一省略。
                            .child(ui::ellipsis(label))
                            .when_some(action, |this, action| {
                                this.on_click(cx.listener(
                                    move |this, _: &ClickEvent, window, cx| {
                                        cx.stop_propagation();
                                        this.run_panel_action(action, window, cx);
                                    },
                                ))
                            })
                            .into_any_element(),
                    }
                }))
                .into_any_element(),
        )
    }

    fn add_rows() -> Vec<PanelRow> {
        // 标题点明落点：「打开文件夹」会新建一页列表，这里的两个动作都只动当前页。
        let mut rows = vec![PanelRow::Title("添加到当前列表")];
        rows.extend(
            [
                ("添加文件…", PanelAction::AddFiles),
                // 说明「含子文件夹」：不然用户会以为它和以前一样只收一层。
                ("添加文件夹（含子文件夹）…", PanelAction::AddFolder),
            ]
            .into_iter()
            .map(|(label, action)| PanelRow::Entry {
                label: label.to_string(),
                active: false,
                action: Some(action),
            }),
        );
        rows
    }

    fn sort_rows(&self) -> Vec<PanelRow> {
        let mode = self.playlist().sort_mode();
        let mut rows = vec![PanelRow::Title("排序方式")];
        for candidate in [SortMode::Name, SortMode::Modified, SortMode::Added] {
            rows.push(PanelRow::Entry {
                label: candidate.menu_label().to_string(),
                active: mode == candidate,
                action: Some(PanelAction::ApplySort(candidate)),
            });
        }
        if mode == SortMode::Manual {
            rows.push(PanelRow::Entry {
                label: "当前：手工排序（拖动行调整）".to_string(),
                active: true,
                action: None,
            });
        }

        rows.push(PanelRow::Divider);
        rows.push(PanelRow::Title("恢复顺序（最近 3 版）"));
        let history = self.playlist().history();
        if history.is_empty() {
            rows.push(PanelRow::Entry {
                label: "还没有可恢复的顺序".to_string(),
                active: false,
                action: None,
            });
        } else {
            for (index, version) in history.iter().enumerate() {
                rows.push(PanelRow::Entry {
                    label: format!(
                        "第 {} 版 · {}（{} 项）",
                        index + 1,
                        version.mode().label(),
                        version.len()
                    ),
                    active: false,
                    action: Some(PanelAction::Restore(index)),
                });
            }
        }

        // 没有文件夹时这两项点了也不会有反应，干脆不占地方。
        if self.playlist().has_dir() {
            rows.push(PanelRow::Divider);
            rows.push(PanelRow::Title("文件夹"));
            rows.push(PanelRow::Entry {
                label: "收起所有文件夹".to_string(),
                active: false,
                action: Some(PanelAction::FoldAll(true)),
            });
            rows.push(PanelRow::Entry {
                label: "展开所有文件夹".to_string(),
                active: false,
                action: Some(PanelAction::FoldAll(false)),
            });
        }
        rows
    }

    fn list_rows(&self) -> Vec<PanelRow> {
        let active = self.playlists.active();
        let mut rows = vec![PanelRow::Title("全部列表")];
        for (index, tab) in self.playlists.tabs().iter().enumerate() {
            // 名字后面带上本页有多少项：光看名字分不清哪一页还有内容。
            let mut label = format!("{}（{} 项）", tab.name, tab.playlist.len());
            if self.playing_tab == Some(index) {
                label.push_str(" · 播放中");
            }
            rows.push(PanelRow::Entry {
                label,
                active: index == active,
                action: Some(PanelAction::SelectTab(index)),
            });
        }
        rows
    }

    fn run_panel_action(
        &mut self,
        action: PanelAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.panel_menu = None;
        match action {
            PanelAction::AddFiles => self.pick_add_files(window, cx),
            PanelAction::AddFolder => self.pick_add_folder(window, cx),
            PanelAction::ApplySort(mode) => self.apply_sort(mode, cx),
            PanelAction::Restore(index) => self.restore_order(index, cx),
            PanelAction::SelectTab(index) => self.select_tab(index, cx),
            PanelAction::FoldAll(folded) => self.set_all_folded(folded, cx),
        }
        cx.notify();
    }
}
