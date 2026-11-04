//! 转换面板里的小件：小节标题、互斥 chip、格式下拉的一行。

use super::super::*;
use crate::convert::TargetFormat;

pub(super) fn section_title(text: &'static str) -> AnyElement {
    div()
        .text_size(px(12.0))
        .text_color(text_muted())
        .child(text)
        .into_any_element()
}

/// 元素 id 要全局唯一；选项标签里带中文，拼一个稳定前缀避免和别处撞车。
pub(super) fn convert_id(prefix: &'static str, label: &'static str) -> SharedString {
    SharedString::from(format!("{prefix}-{label}"))
}

/// 来源切换与编码器档位共用的小按钮。选中那条用强调色填充，一眼看得出当前选择。
pub(super) fn convert_chip(
    id: impl Into<ElementId>,
    label: &'static str,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .h(px(28.0))
        .px(px(10.0))
        .rounded(px(7.0))
        .text_size(px(12.0))
        .cursor(CursorStyle::PointingHand)
        .when(active, |this| {
            this.bg(ui::accent()).text_color(gpui_kit::rgb(0x111114))
        })
        .when(!active, |this| {
            this.bg(ui::surface_hover())
                .text_color(text_muted())
                .hover(|style| style.bg(ui::surface_active()).text_color(text()))
        })
        .child(label)
        .on_click(
            move |event: &ClickEvent, window: &mut Window, cx: &mut App| {
                // 面板底下是视频区，不拦住会顺手切换播放/暂停。
                cx.stop_propagation();
                on_click(event, window, cx);
            },
        )
        .into_any_element()
}

/// 下拉列表里的一行：扩展名 + 编码说明 + 视频/音频标签。
pub(super) fn render_picker_row(
    selected: TargetFormat,
    format: TargetFormat,
    cx: &mut Context<PlayerApp>,
) -> AnyElement {
    let active = selected == format;
    // 选中行底色是强调色，浅色调的文字在上面会糊成一片，统一换成深色。
    let ink: Hsla = if active {
        gpui_kit::rgb(0x111114).into()
    } else {
        text_faint()
    };
    div()
        .id(SharedString::from(format!(
            "convert-format-{}",
            format.extension()
        )))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .h(px(30.0))
        .px(px(8.0))
        .rounded(px(6.0))
        .cursor(CursorStyle::PointingHand)
        .when(active, |this| {
            this.bg(ui::accent()).text_color(gpui_kit::rgb(0x111114))
        })
        .when(!active, |this| {
            this.text_color(text())
                .hover(|style| style.bg(ui::surface_hover()))
        })
        .child(
            div()
                .flex()
                .flex_1()
                .min_w(px(0.0))
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .w(px(44.0))
                        .text_size(px(12.0))
                        .child(format.extension()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(10.0))
                        .text_color(ink)
                        .child(ui::ellipsis(format.hint())),
                ),
        )
        .child(
            div()
                .text_size(px(9.0))
                .text_color(ink)
                .child(format.kind().label()),
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            cx.stop_propagation();
            this.convert_pick_format(format, cx);
        }))
        .into_any_element()
}
