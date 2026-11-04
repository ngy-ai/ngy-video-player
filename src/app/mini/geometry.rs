//! 小窗的纯计算：尺寸与指针坐标换算。
//!
//! 这些函数不碰 `Context`，输入全是普通数值，测试可以直接调（见 `tests.rs`）。
//! 退化情况返回 `None`：几何算不出来时，最合理的回退永远是「保持现状」，
//! 由调用方决定回退到哪个字段。

use super::*;

/// 小窗宽度：用户拖过的值优先，但最多占屏幕宽的 40%——换到更小的屏幕上时，
/// 别让「小窗」变成半个桌面。
pub(in crate::app) fn mini_width(preferred: f32, screen_width: f32) -> f32 {
    let limit = (screen_width * 0.4).max(MINI_MIN_WIDTH);
    preferred.clamp(MINI_MIN_WIDTH, MINI_MAX_WIDTH).min(limit)
}

/// 小窗高宽比：跟着画面设置走，旋转 90/270 度时长宽互换。
pub(in crate::app) fn mini_aspect(ratio: f32, swapped: bool) -> f32 {
    let ratio = if swapped { ratio.recip() } else { ratio };
    ratio.clamp(0.4, 4.0)
}

/// 把窗口在某一轴上的位置换算成 0..1 的比例：0 贴屏幕起始边，1 贴结束边。
///
/// 窗口比屏幕还大时没有可移动的空间，统一按 0 算——否则选一个负数再夹到 0，
/// 每次打开都会偏一点。
pub(in crate::app) fn axis_ratio(
    origin: f32,
    screen_origin: f32,
    screen_span: f32,
    window_span: f32,
) -> f32 {
    let travel = screen_span - window_span;
    if travel <= 1.0 {
        return 0.0;
    }
    ((origin - screen_origin) / travel).clamp(0.0, 1.0)
}

/// 指针横坐标换算成片内秒数。
pub(super) fn time_for_x(bounds: Bounds<Pixels>, x: Pixels, duration: f64) -> Option<f64> {
    let width = f32::from(bounds.size.width);
    if width <= 1.0 || duration <= 0.0 {
        return None;
    }
    let offset = f32::from(x) - f32::from(bounds.origin.x);
    Some((offset as f64 / width as f64).clamp(0.0, 1.0) * duration)
}

/// 指针纵坐标换算成音量百分比。上限与主窗口一致（mpv 能到 130）。
///
/// 竖直滑块是「上大下小」：`Pixels` 的 y 轴向下增长，所以 y 越小音量越大。
pub(super) fn volume_for_y(bounds: Bounds<Pixels>, y: Pixels) -> Option<f64> {
    let height = f32::from(bounds.size.height);
    if height <= 1.0 {
        return None;
    }
    let offset = f32::from(y) - f32::from(bounds.origin.y);
    let ratio = (offset / height).clamp(0.0, 1.0);
    Some((1.0 - ratio as f64) * 130.0)
}
