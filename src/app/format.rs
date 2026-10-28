//! 文本、时间与尺寸格式化。
//!
//! 只做纯字符串处理，不碰播放器状态，渲染代码可以随意复用。

use super::*;

/// 控制条上的倍速文字：`1` → `1×`，`1.25` → `1.25×`。
pub(super) fn format_speed(speed: f64) -> String {
    let mut text = format!("{speed:.2}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    format!("{text}×")
}

/// 自定义滑块气泡上的倍速：整数档写成 `1.0×`，更细的档位照实显示 `1.25×`。
pub(super) fn format_speed_value(speed: f64) -> String {
    let rounded = (speed * 100.0).round() / 100.0;
    if (rounded * 10.0 - (rounded * 10.0).round()).abs() < 0.001 {
        format!("{rounded:.1}×")
    } else {
        format!("{rounded:.2}×")
    }
}

/// 从 mpv 的完整命令错误里取出最后一段（真正的失败原因），放进 OSD 不至于撑满窗口。
pub(super) fn error_reason(message: &str) -> String {
    let reason = message.rsplit(": ").next().unwrap_or(message).trim();
    let mut chars = reason.chars();
    let short: String = chars.by_ref().take(48).collect();
    if chars.next().is_some() {
        format!("{short}…")
    } else {
        short
    }
}

/// 滑块两端的量程标签。固定宽度，滑轨不会因为数字长短而左右跳。
pub(super) fn speed_edge_label(label: &'static str, width: f32) -> impl IntoElement {
    div()
        .w(px(width))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(11.0))
        .text_color(text_faint())
        .child(label)
}

/// `8_400_000_000` → `8.4 GB`。文件大小用二进制单位，和资源管理器说的一致。
pub(super) fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}
