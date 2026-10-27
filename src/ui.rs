//! 界面层的小构件：配色、按钮、时间格式化、视频画面。
//!
//! 这里的控件都是自绘的 `div`，不用组件库的 `Button`——播放器控制条对间距、
//! 悬停反馈、禁用态的要求很具体，用自己的两层 div 比定制组件库样式更短也更稳。

use gpui_kit::component::Icon;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Bounds, ClickEvent, Corners, CursorStyle, ElementId, Hsla, IntoElement,
    Pixels, RenderImage, SharedString, Styled, Window, canvas, div, point, px, rgb, size,
};
use std::sync::Arc;

/// 全量 Lucide 图标（组件库默认只带 101 个常用图标，播放器需要的
/// `repeat`/`list-video`/`volume-x` 不在其中）。
pub use gpui_kit::assets::IconName;

// ---------------------------------------------------------------------------
// 配色：视频播放器一律深色，避免界面比画面更亮影响观感。
// ---------------------------------------------------------------------------

pub const BG: u32 = 0x0a0a0b;
pub const SURFACE: u32 = 0x17171b;
pub const SURFACE_HOVER: u32 = 0x24242a;
pub const SURFACE_ACTIVE: u32 = 0x2f2f37;
pub const TEXT: u32 = 0xededf0;
pub const TEXT_MUTED: u32 = 0x9a9aa4;
pub const TEXT_FAINT: u32 = 0x6a6a74;
pub const ACCENT: u32 = 0x5aa9ff;
pub const DANGER: u32 = 0xff6b6b;
pub const TRACK: u32 = 0x3a3a44;

pub fn bg() -> Hsla {
    rgb(BG).into()
}
pub fn surface() -> Hsla {
    rgb(SURFACE).into()
}
pub fn surface_hover() -> Hsla {
    rgb(SURFACE_HOVER).into()
}
pub fn surface_active() -> Hsla {
    rgb(SURFACE_ACTIVE).into()
}
pub fn text() -> Hsla {
    rgb(TEXT).into()
}
pub fn text_muted() -> Hsla {
    rgb(TEXT_MUTED).into()
}
pub fn text_faint() -> Hsla {
    rgb(TEXT_FAINT).into()
}
pub fn accent() -> Hsla {
    rgb(ACCENT).into()
}
pub fn danger() -> Hsla {
    rgb(DANGER).into()
}
pub fn track() -> Hsla {
    rgb(TRACK).into()
}

/// 控制条背后的压暗层，保证白字在亮画面上也看得清。
pub fn scrim() -> Hsla {
    Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.0,
        a: 0.78,
    }
}

pub fn overlay(alpha: f32) -> Hsla {
    Hsla {
        h: 0.220,
        s: 0.14,
        l: 0.04,
        a: alpha,
    }
}

// ---------------------------------------------------------------------------
// 图标按钮
// ---------------------------------------------------------------------------

pub fn icon(icon: IconName, size: f32, color: Hsla) -> impl IntoElement {
    Icon::new(icon).size(px(size)).text_color(color)
}

/// 控制条上的图标按钮。`active` 用于「已开启」的开关态（字幕、循环等）。
pub fn icon_button(
    id: impl Into<ElementId>,
    icon_name: IconName,
    active: bool,
    disabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let color = if disabled {
        text_faint()
    } else if active {
        accent()
    } else {
        text()
    };

    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .size(px(34.0))
        .rounded(px(8.0))
        .cursor(if disabled {
            CursorStyle::Arrow
        } else {
            CursorStyle::PointingHand
        })
        .when(!disabled, |this| {
            this.hover(|style| style.bg(surface_hover()).rounded(px(8.0)))
        })
        .child(icon(icon_name, 18.0, color))
        .when(!disabled, |this| {
            this.on_click(
                move |event: &ClickEvent, window: &mut Window, cx: &mut App| {
                    // 同一位置命中的 hitbox 会**全部**收到这次点击，控制条上的按钮和
                    // 底下的画面区域算两个兄弟节点，都会被派发到。不回吞的话，点「字幕」
                    // 会先打开菜单、紧接着被画面区域的单击逻辑关掉——表现就是按钮点了
                    // 没反应。这里先截断，控制条内部的点击不再往画面区域冒泡。
                    cx.stop_propagation();
                    on_click(event, window, cx);
                },
            )
        })
        .into_any_element()
}

/// 带文字的图标按钮：给「当前值本身要让人看见」的开关用（倍速、播放模式）。
///
/// 纯图标按钮在这些功能上不够用——用户看不到「现在是不是 1.5 倍速」「现在是列表
/// 循环还是单集循环」，就会以为播放器根本没有这些功能。
pub fn pill_button(
    id: impl Into<ElementId>,
    icon_name: IconName,
    label: impl Into<SharedString>,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let color = if active { accent() } else { text() };

    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(4.0))
        .h(px(28.0))
        .px(px(7.0))
        .rounded(px(8.0))
        .cursor(CursorStyle::PointingHand)
        .hover(|style| style.bg(surface_hover()).rounded(px(8.0)))
        .child(icon(icon_name, 15.0, color))
        .child(
            div()
                .text_size(px(11.0))
                .text_color(color)
                .child(label.into()),
        )
        .on_click(
            move |event: &ClickEvent, window: &mut Window, cx: &mut App| {
                cx.stop_propagation();
                on_click(event, window, cx);
            },
        )
        .into_any_element()
}

// ---------------------------------------------------------------------------
// 时间
// ---------------------------------------------------------------------------

/// `4832.5` → `1:20:32`；`125` → `2:05`。负数按 `-` 前缀处理（剩余时间）。
pub fn format_time(seconds: f64) -> String {
    if !seconds.is_finite() {
        return "--:--".to_string();
    }
    let negative = seconds < 0.0;
    let total = seconds.abs().round() as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let secs = total % 60;
    let text = if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes}:{secs:02}")
    };
    if negative { format!("-{text}") } else { text }
}

// ---------------------------------------------------------------------------
// 视频画面
// ---------------------------------------------------------------------------

/// 在 `bounds` 里按 `aspect` 等比居中放一个矩形（上下或左右留黑边）。
pub fn fit_rect(bounds: Bounds<Pixels>, aspect: f32) -> Bounds<Pixels> {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width <= 0.0 || height <= 0.0 || !aspect.is_finite() || aspect <= 0.0 {
        return bounds;
    }
    let (w, h) = if width / height > aspect {
        (height * aspect, height)
    } else {
        (width, width / aspect)
    };
    Bounds::new(
        point(
            bounds.origin.x + px((width - w) * 0.5),
            bounds.origin.y + px((height - h) * 0.5),
        ),
        size(px(w), px(h)),
    )
}

/// 视频画面本身。
///
/// 用 `canvas` 而不是 `img()`：`img()` 走的是「按图片固有尺寸渲染」的路径，
/// 这里我们更希望画面精确铺满视频区域——等比与黑边已经在 mpv 那侧排好了。
pub fn video_surface(frame: Option<Arc<RenderImage>>) -> AnyElement {
    canvas(
        |bounds, _window, _cx| bounds,
        move |_bounds, target, window, _cx| {
            let Some(image) = frame.as_ref() else {
                return;
            };
            // 帧里已经按视频比例排好黑边，正常情况下比例和区域一致，等比之后
            // 一像素都不差。但区域刚变过（收起/展开播放列表、拖窗口）而新帧
            // 还没送到时，手上这帧还是旧比例：直接铺满会把画面拉扁，所以退一步
            // 按帧自身的比例居中，宁可两侧多一条黑边也不让画面变形。
            let size = image.size(0);
            let (width, height) = (i32::from(size.width) as f32, i32::from(size.height) as f32);
            let fitted = fit_rect(target, width / height);
            let _ = window.paint_image(fitted, fitted, Corners::default(), image.clone(), 0, false);
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

// ---------------------------------------------------------------------------
// 杂项
// ---------------------------------------------------------------------------

pub fn label(value: impl Into<SharedString>, size: f32, color: Hsla) -> impl IntoElement {
    div()
        .text_size(px(size))
        .text_color(color)
        .child(value.into())
}

/// 单行省略，用于播放列表项。
pub fn ellipsis(value: impl Into<SharedString>) -> impl IntoElement {
    div()
        .w_full()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(value.into())
}
