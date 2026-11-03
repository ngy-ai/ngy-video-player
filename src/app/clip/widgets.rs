//! 裁切面板里的小控件：分段标题、说明文字、互斥选项按钮、复选框行、步进按钮。
//!
//! 视频选项与音频选项分在两个文件里，但排版用的是同一套原子控件；放在这里
//! 两边共用，免得一个改字号另一个不动。

use super::*;
use crate::clip::ExportOptions;

/// 一段选项的小标题。
pub(super) fn section_title(label: &'static str) -> AnyElement {
    div()
        .pt(px(2.0))
        .text_size(px(11.0))
        .text_color(text_muted())
        .child(label)
        .into_any_element()
}

/// 标题下面那行灰色说明。
pub(super) fn note(text: &'static str) -> AnyElement {
    note_text(text.to_string())
}

/// 内容要拼出来的说明（比如带上轨道名）。
pub(super) fn note_owned(text: String) -> AnyElement {
    note_text(text)
}

fn note_text(text: String) -> AnyElement {
    div()
        .text_size(px(10.0))
        .text_color(text_faint())
        .whitespace_normal()
        .child(text)
        .into_any_element()
}

/// 一组互斥选项里的一颗小按钮。选中项用强调色填充。
pub(super) fn option_chip(
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
        .h(px(26.0))
        .px(px(9.0))
        .rounded(px(6.0))
        .text_size(px(11.0))
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

/// 一排互斥选项。画面变换、采样率、声道用的是同一种铺法，只有类型和回调不同。
pub(super) fn chip_row<T>(
    prefix: &'static str,
    items: &[T],
    current: T,
    label: fn(T) -> &'static str,
    set: fn(&mut ExportOptions, T),
    cx: &mut Context<PlayerApp>,
) -> AnyElement
where
    T: Copy + PartialEq + 'static,
{
    div()
        .flex()
        // 窄面板里 5 个选项排不下就换行，别把最后一个挤到面板外面。
        .flex_wrap()
        .items_center()
        .gap(px(6.0))
        .children(items.iter().map(|&item| {
            option_chip(
                static_id(prefix, label(item)),
                label(item),
                current == item,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.update_clip_options(|options| set(options, item), cx);
                }),
            )
        }))
        .into_any_element()
}

/// 复选框一行。`update` 只负责翻转字段，重绘与取值由调用方统一处理。
pub(super) fn toggle_row(
    id: &'static str,
    label: &'static str,
    selected: bool,
    update: fn(&mut PlayerApp, &mut Context<PlayerApp>),
    cx: &mut Context<PlayerApp>,
) -> AnyElement {
    picture_option(
        id,
        label,
        selected,
        true,
        cx.listener(move |this, _: &ClickEvent, _window, cx| {
            update(this, cx);
        }),
    )
}

/// 元素 id 要全局唯一；选项标签里带符号与空格，拼一个稳定前缀避免和别处撞车。
pub(super) fn static_id(prefix: &'static str, label: &'static str) -> SharedString {
    SharedString::from(format!("{prefix}-{label}"))
}

/// 24px 的方形小按钮，专给裁切那几行用：设置窗口那套按钮 32px 高、带内边距，
/// 四个并排就把面板撑爆了。
pub(super) fn stepper_button(
    id: impl Into<ElementId>,
    label: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(24.0))
        .rounded(px(6.0))
        .bg(ui::surface_hover())
        .text_size(px(13.0))
        .text_color(text_muted())
        .cursor(CursorStyle::PointingHand)
        .hover(|style| style.bg(ui::surface_active()).text_color(text()))
        .child(label)
        .on_click(
            move |event: &ClickEvent, window: &mut Window, cx: &mut App| {
                cx.stop_propagation();
                on_click(event, window, cx);
            },
        )
        .into_any_element()
}
