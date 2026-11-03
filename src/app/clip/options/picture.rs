//! 画面变换与画面处理：裁切、缩放、补边、旋转、翻转、色彩、降噪、插帧、倒放、
//! 开头定格与文字水印。
//!
//! 只在重编码时出现（流复制没有像素可以重排）。顺序按「先修时间/色彩、再动几何、
//! 再修噪点、最后锐化、最后压字幕与水印」排，和 `clip::export` 里拼滤镜链的顺序
//! 一致，面板上看到的就是实际执行顺序。

use super::super::widgets::{chip_row, note, section_title, stepper_button, toggle_row};
use super::super::*;
use super::controls::{StepperTarget, step_row};
use crate::clip::{
    ColorMatrix, ColorRange, Crop, Edge, ExportOptions, FrameRate, Pad, Rotate, Scale,
    WatermarkPosition,
};
use gpui_kit::component::input::Input;

impl PlayerApp {
    pub(super) fn render_clip_picture(
        &self,
        dialog: &ClipDialog,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let options = &dialog.options;
        if !options.filters_available() {
            return None;
        }

        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                // 画面变换排在滤镜之前：裁掉黑边、缩到目标尺寸之后再降噪/锐化，
                // 既省时间，也不会把黑边一起锐化出硬边。
                .child(section_title("画面变换"))
                .child(crop_controls(options.transform.crop, cx))
                .child(chip_row(
                    "clip-scale",
                    &Scale::ALL,
                    options.transform.scale,
                    Scale::label,
                    |options, scale| options.transform.scale = scale,
                    cx,
                ))
                .child(note(options.transform.scale.hint()))
                .child(chip_row(
                    "clip-pad",
                    &Pad::ALL,
                    options.transform.pad,
                    Pad::label,
                    |options, pad| options.transform.pad = pad,
                    cx,
                ))
                .child(note(options.transform.pad.hint()))
                .child(chip_row(
                    "clip-rotate",
                    &Rotate::ALL,
                    options.transform.rotate,
                    Rotate::label,
                    |options, rotate| options.transform.rotate = rotate,
                    cx,
                ))
                .child(toggle_row(
                    "clip-option-flip-h",
                    "水平翻转",
                    options.transform.flip_h,
                    |this, cx| {
                        this.update_clip_options(|options| options.transform.flip_h ^= true, cx);
                    },
                    cx,
                ))
                .child(toggle_row(
                    "clip-option-flip-v",
                    "垂直翻转",
                    options.transform.flip_v,
                    |this, cx| {
                        this.update_clip_options(|options| options.transform.flip_v ^= true, cx);
                    },
                    cx,
                ))
                .child(section_title("画面处理"))
                .child(toggle_row(
                    "clip-option-tonemap",
                    "HDR → SDR",
                    options.tonemap_hdr,
                    |this, cx| {
                        this.update_clip_options(|options| options.tonemap_hdr ^= true, cx);
                    },
                    cx,
                ))
                // zscale 依赖片源里的色彩标签（primaries / transfer）。无标签的
                // 源上整条链会报「no path between colorspaces」，只能提示用户
                // 别在普通 SDR 片源上开这个开关。
                .child(note(
                    "「HDR → SDR」只用于带色彩标签的 HDR 片源；普通 SDR 片源不要开，画面会偏色。",
                ))
                .child(toggle_row(
                    "clip-option-deinterlace",
                    "去隔行",
                    options.deinterlace,
                    |this, cx| {
                        this.update_clip_options(|options| options.deinterlace ^= true, cx);
                    },
                    cx,
                ))
                .child(toggle_row(
                    "clip-option-stabilize",
                    "防抖（手持素材）",
                    options.stabilize,
                    |this, cx| {
                        this.update_clip_options(|options| options.stabilize ^= true, cx);
                    },
                    cx,
                ))
                .child(note(
                    "单遍版本，能把小幅抖动压平；大幅晃动只能减轻，画面边缘会略微软化。",
                ))
                .child(toggle_row(
                    "clip-option-denoise",
                    "降噪",
                    options.denoise,
                    |this, cx| {
                        this.update_clip_options(|options| options.denoise ^= true, cx);
                    },
                    cx,
                ))
                .child(toggle_row(
                    "clip-option-sharpen",
                    "锐化",
                    options.sharpen,
                    |this, cx| {
                        this.update_clip_options(|options| options.sharpen ^= true, cx);
                    },
                    cx,
                ))
                .child(toggle_row(
                    "clip-option-cfr",
                    "恒定帧率（修复 VFR 抖动）",
                    options.force_cfr,
                    |this, cx| {
                        this.update_clip_options(|options| options.force_cfr ^= true, cx);
                    },
                    cx,
                ))
                .child(toggle_row(
                    "clip-option-interpolate",
                    "光流插帧",
                    options.interpolate,
                    |this, cx| {
                        this.update_clip_options(|options| options.interpolate ^= true, cx);
                    },
                    cx,
                ))
                .when(options.interpolate, |this| {
                    this.child(chip_row(
                        "clip-interpolate-fps",
                        &FrameRate::ALL,
                        options.interpolate_fps,
                        FrameRate::label,
                        |options, rate| options.interpolate_fps = rate,
                        cx,
                    ))
                })
                .child(note(
                    "靠运动估计补出中间帧：慢镜头和动画更顺，快速剪辑会出现涂抹；这一步很慢。",
                ))
                // 倒放与定格改的是帧的先后顺序，和上面的画质处理不是一类事，
                // 单独开一段；两项都只在重编码时可用，正好都在这个分支里。
                .child(section_title("时间"))
                .child(toggle_row(
                    "clip-option-reverse",
                    "倒放",
                    options.reverse,
                    |this, cx| {
                        this.update_clip_options(|options| options.reverse ^= true, cx);
                    },
                    cx,
                ))
                .child(note(
                    "整段从后往前播，画面与声音都会重编码；倒放要把整段帧攒在内存里，只适合短片。",
                ))
                .child(step_row(
                    "clip-freeze",
                    "开头定格",
                    freeze_label(options.freeze_seconds()),
                    "把首帧冻住这么久再开始播，用来做标题卡；声音同步推迟。点数值归零。",
                    StepperTarget::Freeze,
                    cx,
                ))
                .child(watermark_controls(dialog, options, cx))
                // 色彩放在画面处理之后：色调映射、范围与矩阵都属于「最后一步定调」，
                // 排在一起用户才看得出它们是同一类操作。
                .child(section_title("色彩"))
                .child(chip_row(
                    "clip-color-matrix",
                    &ColorMatrix::ALL,
                    options.color_matrix,
                    ColorMatrix::label,
                    |options, matrix| options.color_matrix = matrix,
                    cx,
                ))
                .child(note(options.color_matrix.hint()))
                .child(chip_row(
                    "clip-color-range",
                    &ColorRange::ALL,
                    options.color_range,
                    ColorRange::label,
                    |options, range| options.color_range = range,
                    cx,
                ))
                .child(note(options.color_range.hint()))
                .child(toggle_row(
                    "clip-option-tag-color",
                    "按 BT.709 重写色彩标签",
                    options.tag_rec709,
                    |this, cx| {
                        this.update_clip_options(|options| options.tag_rec709 ^= true, cx);
                    },
                    cx,
                ))
                .child(note(
                    "只改标签不动像素，用来修标签写错的源；可以和上面的转换同时用",
                ))
                .child(self.render_clip_audio_options(options, cx))
                .into_any_element(),
        )
    }

    fn nudge_crop(&mut self, edge: Edge, delta: f32, cx: &mut Context<Self>) {
        self.update_clip_options(|options| options.transform.crop.nudge(edge, delta), cx);
    }
}

/// 定格时长的显示文本。0 不说「0 秒」，说「不定格」：面板上所有开关的
/// 关闭态都用同一句话，用户不必记哪个数字代表关。
fn freeze_label(seconds: f32) -> String {
    if seconds <= 0.0 {
        "不定格".to_string()
    } else {
        format!("{seconds:.1} 秒")
    }
}

/// 文字水印：勾选框、文字输入、落点、字号、不透明度。
///
/// 没勾的时候只留一行开关，把整段控件收起来：面板已经很长，
/// 而绝大多数导出都用不到水印。
fn watermark_controls(
    dialog: &ClipDialog,
    options: &ExportOptions,
    cx: &mut Context<PlayerApp>,
) -> AnyElement {
    let mut body = div().flex().flex_col().gap(px(6.0)).child(toggle_row(
        "clip-option-watermark",
        "文字水印",
        options.watermark.enabled,
        |this, cx| {
            this.update_clip_options(|options| options.watermark.enabled ^= true, cx);
        },
        cx,
    ));

    if !options.watermark.enabled {
        return body
            .child(note("把一行文字压在画面上，用来标注来源或版权。"))
            .into_any_element();
    }

    body = body
        .child(
            div().child(
                Input::new(&dialog.watermark_text)
                    .id("clip-watermark-text")
                    .w_full(),
            ),
        )
        .child(chip_row(
            "clip-watermark-pos",
            &WatermarkPosition::ALL,
            options.watermark.position,
            WatermarkPosition::label,
            |options, position| options.watermark.position = position,
            cx,
        ))
        .child(step_row(
            "clip-watermark-size",
            "字号",
            format!("{} px", options.watermark.size),
            "按输出画面的像素算：1080p 用 28 左右，4K 素材要调到 50 以上。",
            StepperTarget::FontSize,
            cx,
        ))
        .child(step_row(
            "clip-watermark-opacity",
            "不透明度",
            format!("{:.0}%", options.watermark.opacity * 100.0),
            "半透明的水印不挡画面，但压在高光上会看不清；配了淡阴影兜底。",
            StepperTarget::Opacity,
            cx,
        ))
        .child(note(
            "文字留空时不会导出；用的是系统里的中文字体，找不到字体文件时交给 ffmpeg 自己挑。",
        ));
    body.into_any_element()
}

/// 裁切四边：上下两行各放一对，四个控件才塞得进右侧面板。
fn crop_controls(crop: Crop, cx: &mut Context<PlayerApp>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(crop_edge(Edge::Left, crop.left, cx))
                .child(crop_edge(Edge::Right, crop.right, cx)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(crop_edge(Edge::Top, crop.top, cx))
                .child(crop_edge(Edge::Bottom, crop.bottom, cx)),
        )
        .child(
            div()
                .text_size(px(10.0))
                .text_color(text_faint())
                .child("裁切：四边各裁掉画面宽/高的百分比，点数值归零；与「补边」是两回事。"),
        )
        .into_any_element()
}

/// 一条边的步进器：`−  10%  +`。
///
/// 用 1% 的步进而不是滑块：面板宽度下滑块拖不准，而黑边的宽度往往就差一两个
/// 百分点；数值本身可点，用来一键归零。
fn crop_edge(edge: Edge, value: f32, cx: &mut Context<PlayerApp>) -> AnyElement {
    let label = edge.label();
    div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .items_center()
        .gap(px(4.0))
        .child(
            div()
                .w(px(16.0))
                .text_size(px(11.0))
                .text_color(text_muted())
                .child(label),
        )
        .child(stepper_button(
            SharedString::from(format!("clip-crop-{label}-less")),
            "-",
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.nudge_crop(edge, -Crop::STEP, cx);
            }),
        ))
        .child(
            div()
                .id(SharedString::from(format!("clip-crop-{label}-value")))
                .flex_1()
                .min_w(px(0.0))
                .h(px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.0))
                .bg(ui::surface_active())
                .text_size(px(11.0))
                .text_color(ui::accent())
                .cursor(CursorStyle::PointingHand)
                .hover(|style| style.bg(ui::surface_hover()))
                .child(format!("{value:.0}%"))
                .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                    this.update_clip_options(
                        |options| options.transform.crop = Crop::default(),
                        cx,
                    );
                })),
        )
        .child(stepper_button(
            SharedString::from(format!("clip-crop-{label}-more")),
            "+",
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.nudge_crop(edge, Crop::STEP, cx);
            }),
        ))
        .into_any_element()
}
