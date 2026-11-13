//! 工程预览上的字幕安全框（M10-T16）：两圈参考线，帮着把字幕放在不会被裁掉的地方。
//!
//! 外圈是动作安全区（90%，画面主体别越出去），内圈是字幕安全区（80%，文字都该在
//! 里面）。比例是行业里通用的近似值，不需要跟序列分辨率 / 帧率挂钩，所以按画面矩形的
//! 百分比定位：缩放窗口、换序列规格都不用另算一遍。
//!
//! 只在**工程模式的预览**里画：它是界面叠层，不进导出的成片、不进截图（截图取的是
//! 解码帧），也不跟着字幕文件走。播放模式看片时不打扰。

use super::*;

/// 两圈参考线：比例、名字、边框颜色（`0xRRGGBBAA`）。
const GUIDES: [(f32, &str, u32); 2] = [
    (0.90, "动作安全区 90%", 0x66e0_ff88),
    (0.80, "字幕安全区 80%", 0xffd5_4aaa),
];

/// 一个安全框在画面矩形里的相对位置（全部是 0..1 的比例）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GuideRect {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

/// 按「占画面宽高各几成」算位置。两边对称留白，所以外圈一定套着内圈。
pub fn guide_rect(ratio: f32) -> GuideRect {
    // `f32::clamp` 遇到 NaN 会把 NaN 带下去，画出来的框就没了；宁可退化成整幅画面。
    let ratio = if ratio.is_finite() {
        ratio.clamp(0.05, 1.0)
    } else {
        1.0
    };
    let inset = (1.0 - ratio) * 0.5;
    GuideRect {
        left: inset,
        top: inset,
        width: ratio,
        height: ratio,
    }
}

/// 盖在画面上的两圈参考线。没有点击处理：点击仍然落到视频区那一层。
pub(super) fn safe_area_layer() -> impl IntoElement {
    div()
        .id("subtitle-safe-area")
        .absolute()
        .inset_0()
        .children(GUIDES.iter().map(|(ratio, label, color)| {
            let rect = guide_rect(*ratio);
            div()
                .absolute()
                .left(gpui_kit::relative(rect.left))
                .top(gpui_kit::relative(rect.top))
                .w(gpui_kit::relative(rect.width))
                .h(gpui_kit::relative(rect.height))
                .border_1()
                .border_color(gpui_kit::rgba(*color))
                .child(
                    div()
                        .absolute()
                        .left(px(4.0))
                        .top(px(2.0))
                        .px(px(3.0))
                        .text_size(px(11.0))
                        .text_color(gpui_kit::rgba(*color))
                        .bg(gpui_kit::rgba(0x0000_0066))
                        .child(*label),
                )
        }))
}

impl PlayerApp {
    /// 「视图 > 字幕安全框」与设置页共用这一处：改完立刻落盘并重绘。
    pub(crate) fn toggle_safe_area(&mut self, cx: &mut Context<Self>) {
        self.settings.show_safe_area = !self.settings.show_safe_area;
        self.settings.save();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(left: f32, right: f32) {
        assert!((left - right).abs() < 1e-5, "{left} != {right}");
    }

    #[test]
    fn 两圈参考线都对称且内圈套在外圈里() {
        let outer = guide_rect(GUIDES[0].0);
        let inner = guide_rect(GUIDES[1].0);
        close(outer.left, 0.05);
        close(outer.top, 0.05);
        close(outer.width, 0.9);
        close(inner.left, 0.1);
        close(inner.top, 0.1);
        close(inner.width, 0.8);
        assert!(outer.left <= inner.left);
        assert!(outer.width >= inner.width);
        close(outer.left * 2.0 + outer.width, 1.0);
        close(inner.top * 2.0 + inner.height, 1.0);
    }

    #[test]
    fn 坏比例夹到画面内() {
        let zero = guide_rect(0.0);
        close(zero.left, 0.475);
        close(zero.width, 0.05);
        let full = guide_rect(4.0);
        close(full.left, 0.0);
        close(full.width, 1.0);
        close(guide_rect(f32::NAN).width, 1.0);
        close(guide_rect(f32::INFINITY).width, 1.0);
    }
}
