//! 裁切面板里的步进器：`−  当前值  +`，点数值回到默认。
//!
//! 开头定格、水印字号、不透明度、切分段大小共用同一套加减与归位逻辑：四行控件
//! 长得一样、只有步长与量程不同，做成一个函数，免得改了字号那行忘了改切分那行。

use super::super::widgets::{note, stepper_button};
use super::super::*;
use crate::clip::{DEFAULT_SPLIT_MB, ExportOptions, MAX_FREEZE_SECONDS, Watermark};

/// 步进器当前在调哪个值。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StepperTarget {
    /// 开头定格，0.5 秒一格。
    Freeze,
    /// 水印字号，4px 一格。
    FontSize,
    /// 水印不透明度，10% 一格。
    Opacity,
    /// 按大小切分的段大小，100MB 一格。
    SplitSize,
}

pub(super) fn apply_step(options: &mut ExportOptions, target: StepperTarget, delta: i32) {
    match target {
        StepperTarget::Freeze => {
            // 存的是秒，按 0.5 秒的整数倍走，界面上的数字才不会变成 1.7000001。
            let halves = (options.freeze_seconds() * 2.0).round() as i32 + delta;
            let halves = halves.clamp(0, (MAX_FREEZE_SECONDS * 2.0) as i32);
            options.freeze_lead = halves as f32 / 2.0;
        }
        StepperTarget::FontSize => {
            let next = options.watermark.size as i32 + delta * Watermark::SIZE_STEP;
            options.watermark.size =
                next.clamp(Watermark::MIN_SIZE as i32, Watermark::MAX_SIZE as i32) as u32;
        }
        StepperTarget::Opacity => {
            let next = (options.watermark.opacity * 100.0).round() as i32
                + delta * Watermark::OPACITY_STEP;
            options.watermark.opacity = next.clamp(5, 100) as f32 / 100.0;
        }
        StepperTarget::SplitSize => {
            let next = options.split_mb as i32 + delta * 100;
            options.split_mb = next.clamp(100, 8_000) as u32;
        }
    }
}

pub(super) fn reset_value(options: &mut ExportOptions, target: StepperTarget) {
    match target {
        StepperTarget::Freeze => options.freeze_lead = 0.0,
        StepperTarget::FontSize => options.watermark.size = Watermark::default().size,
        StepperTarget::Opacity => options.watermark.opacity = Watermark::default().opacity,
        StepperTarget::SplitSize => options.split_mb = DEFAULT_SPLIT_MB,
    }
}

/// 一行步进器：`−  当前值  +`，点数值回到默认。
pub(super) fn step_row(
    prefix: &'static str,
    label: &'static str,
    value: String,
    hint: &'static str,
    target: StepperTarget,
    cx: &mut Context<PlayerApp>,
) -> AnyElement {
    let change = |delta: i32| {
        cx.listener(move |this, _: &ClickEvent, _window, cx| {
            this.update_clip_options(|options| apply_step(options, target, delta), cx);
        })
    };

    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(div().text_size(px(12.0)).text_color(text()).child(label))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(stepper_button(
                            SharedString::from(format!("{prefix}-less")),
                            "-",
                            change(-1),
                        ))
                        .child(
                            div()
                                .id(SharedString::from(format!("{prefix}-value")))
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(76.0))
                                .h(px(24.0))
                                .rounded(px(6.0))
                                .bg(ui::surface_active())
                                .text_size(px(11.0))
                                .text_color(ui::accent())
                                .cursor(CursorStyle::PointingHand)
                                .hover(|style| style.bg(ui::surface_hover()))
                                .child(value)
                                .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                    cx.stop_propagation();
                                    this.update_clip_options(
                                        |options| reset_value(options, target),
                                        cx,
                                    );
                                })),
                        )
                        .child(stepper_button(
                            SharedString::from(format!("{prefix}-more")),
                            "+",
                            change(1),
                        )),
                ),
        )
        .child(note(hint))
        .into_any_element()
}
