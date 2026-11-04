//! 小窗顶部悬浮条：标题、置顶、还原、退出。

use super::*;

impl MiniPlayer {
    /// 顶部悬浮条：标题 + 置顶 + 还原 + 关闭。
    pub(super) fn render_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("mini-bar")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(32.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .px(px(8.0))
            .bg(ui::scrim())
            // 点悬浮条的空白处不该穿透到底下的画面上去触发「单击暂停」。
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.raise_to_top(window);
                }),
            )
            .child(
                // 拖标题移动窗口。这里必须 stop_propagation，否则会被下面的画面
                // 当成一次单击而顺手把播放暂停掉。
                div()
                    .id("mini-title")
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(12.0))
                    .text_color(text())
                    .cursor(CursorStyle::PointingHand)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.raise_to_top(window);
                            window.start_window_move();
                        }),
                    )
                    .child(self.state.title.clone()),
            )
            .child(icon_button_with_tip(
                "mini-on-top",
                // 两个图标而不是同一个图标变色：置顶与否只看颜色的差别太弱，
                // 尤其在视频画面上什么底色都可能出现。
                if self.on_top {
                    IconName::Pin
                } else {
                    IconName::PinOff
                },
                self.on_top,
                false,
                // 提示写「点了会发生什么」，已置顶时就不该再说「置顶」。
                Some(if self.on_top {
                    "取消置顶"
                } else {
                    "窗口置顶"
                }),
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.toggle_on_top(window, cx);
                }),
            ))
            .child(icon_button_with_tip(
                "mini-restore",
                IconName::Minimize2,
                false,
                false,
                Some("回到主窗口"),
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.restore_to_main(window, cx);
                }),
            ))
            .child(icon_button_with_tip(
                "mini-close",
                IconName::X,
                false,
                false,
                // 悬浮条上的关闭和主窗口标题栏上的关闭是同一件事：结束整个应用。
                // 「回到大窗口」是左边那个还原按钮的职责，两者不能混。
                Some("退出程序"),
                cx.listener(|_this, _: &ClickEvent, _window, cx| cx.quit()),
            ))
            .into_any_element()
    }
}
