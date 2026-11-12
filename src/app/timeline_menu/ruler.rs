//! 标尺右键菜单：整条时间线级别的动作，不指向任何片段。

use super::*;

impl PlayerApp {
    /// 标尺菜单。外壳与片段菜单一致，只是内容只有时间线级别的那几项。
    pub(super) fn render_ruler_menu(
        &self,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let project = self.project.as_ref()?;
        // 没设过工作区时这一项压暗，点下去给一句说明而不是无声无息。
        let has_range = project.ui.in_point.is_some() || project.ui.out_point.is_some();
        let origin = menu_origin(position, menu_height(1, 0), window);

        Some(
            div()
                .id("timeline-ruler-menu")
                .absolute()
                .left(origin.x)
                .top(origin.y)
                .w(px(ITEM_MENU_WIDTH))
                .py(px(6.0))
                .rounded(px(10.0))
                .bg(surface())
                .border_1()
                .border_color(gpui_kit::rgb(0x2c2c34))
                .shadow_lg()
                // 与片段菜单一样：点菜单本身不能漏到下面的标尺去挪播放头。
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
                    "timeline-ruler-clear-range",
                    IconName::Trash,
                    "清除工作区",
                    if has_range { text() } else { text_faint() },
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.timeline_menu = None;
                        this.clear_work_area(cx);
                    }),
                ))
                .into_any_element(),
        )
    }
}
