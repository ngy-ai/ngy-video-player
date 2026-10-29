//! 播放列表面板标题栏上的弹出菜单：添加（文件 / 文件夹）与排序。
//!
//! 两个菜单共用一套行结构和一个浮层，位置也一样，所以放在同一个模块里；
//! 菜单里真正干活的逻辑还留在各自的模块（添加在 `playlist_ops`，排序在
//! `playlist_sort`）。

use super::*;
use crate::playlist::SortMode;

/// 排序菜单离面板顶端的距离。标题行是 34 的按钮加 8+8 的内边距，正好 50，
/// 再让开 2 像素，菜单就像从按钮下面拉出来的一样。
const MENU_TOP: f32 = 52.0;

/// 面板标题栏上展开着的是哪个菜单。用 `Option` 而不是两个 bool：
/// 同一个位置只可能有一个浮层，状态里就不该能表示出「两个都开着」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PanelMenu {
    Add,
    Sort,
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
            Some("添加文件或文件夹"),
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.toggle_panel_menu(PanelMenu::Add, cx);
            }),
        )
    }

    pub(super) fn toggle_panel_menu(&mut self, menu: PanelMenu, cx: &mut Context<Self>) {
        self.panel_menu = (self.panel_menu != Some(menu)).then_some(menu);
        self.poke();
        cx.notify();
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
        };

        Some(
            div()
                .id("playlist-panel-menu")
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
                            .child(label)
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
        [
            ("添加文件…", PanelAction::AddFiles),
            ("添加文件夹…", PanelAction::AddFolder),
        ]
        .into_iter()
        .map(|(label, action)| PanelRow::Entry {
            label: label.to_string(),
            active: false,
            action: Some(action),
        })
        .collect()
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
        }
        cx.notify();
    }
}
