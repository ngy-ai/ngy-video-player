//! 小窗右键菜单：置顶、回到主窗口、退出。
//!
//! 小窗没有菜单栏，悬浮条上的图标又要先把指针停住才出现；右键菜单不占
//! 控制栏宽度，也符合迷你播放器的习惯。

use super::*;

/// 菜单宽度，用来把弹出位置夹在窗口内。
const MENU_WIDTH: f32 = 160.0;
/// 一项的高度（含上下内边距）与菜单整体的上下留白，同样用于夹位置。
const MENU_ITEM_HEIGHT: f32 = 30.0;
const MENU_PADDING: f32 = 6.0;

impl MiniPlayer {
    /// 右键菜单。弹在指针位置，越界时往窗口内收。
    pub(super) fn render_menu(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(position) = self.menu else {
            return div().into_any_element();
        };
        let height = MENU_ITEM_HEIGHT * 4.0 + MENU_PADDING * 2.0;
        let viewport = window.viewport_size();
        let max_left = (f32::from(viewport.width) - MENU_WIDTH - 8.0).max(0.0);
        let max_top = (f32::from(viewport.height) - height - 8.0).max(0.0);
        let left = f32::from(position.x).clamp(0.0, max_left);
        let top = f32::from(position.y).clamp(0.0, max_top);

        div()
            .id("mini-menu")
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(MENU_WIDTH))
            .py(px(MENU_PADDING))
            .rounded(px(10.0))
            .bg(surface())
            .border_1()
            .border_color(gpui_kit::rgb(0x2c2c34))
            .shadow_lg()
            // 点在菜单底板的空白处也不该冒泡成「点画面暂停」。
            .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                cx.stop_propagation();
            }))
            .child(self.menu_item(
                "mini-menu-on-top",
                if self.on_top {
                    "取消置顶"
                } else {
                    "窗口置顶"
                },
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.menu = None;
                    this.toggle_on_top(window, cx);
                }),
            ))
            .child(self.menu_item(
                "mini-menu-loop",
                match self.state.loop_mode {
                    LoopMode::Off => "循环：顺序",
                    LoopMode::All => "循环：列表",
                    LoopMode::One => "循环：单集",
                },
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    // 与主窗口的菜单同一顺序：顺序 → 列表 → 单集 → 顺序。
                    let next = match this.state.loop_mode {
                        LoopMode::Off => LoopMode::All,
                        LoopMode::All => LoopMode::One,
                        LoopMode::One => LoopMode::Off,
                    };
                    this.menu = None;
                    this.forward(cx, move |app, cx| app.set_loop(next, cx));
                }),
            ))
            .child(self.menu_item(
                "mini-menu-restore",
                "回到主窗口",
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.menu = None;
                    this.restore_to_main(window, cx);
                }),
            ))
            .child(self.menu_item(
                "mini-menu-quit",
                "退出程序",
                cx.listener(|_this, _: &ClickEvent, _window, cx| cx.quit()),
            ))
            .into_any_element()
    }

    fn menu_item<L: IntoElement + 'static>(
        &self,
        id: &'static str,
        label: L,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> AnyElement {
        div()
            .id(id)
            .px(px(10.0))
            .py(px(7.0))
            .mx(px(4.0))
            .rounded(px(6.0))
            .text_size(px(12.0))
            .text_color(text())
            .cursor(CursorStyle::PointingHand)
            .hover(|style| style.bg(ui::surface_hover()))
            .on_click(on_click)
            .child(label)
            .into_any_element()
    }
}
