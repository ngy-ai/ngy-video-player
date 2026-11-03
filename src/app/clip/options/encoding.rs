//! 编码与画质：编码器档位、码率控制、画质档、像素格式。
//!
//! 只在重编码时出现：流复制没有任何编码参数可调（GIF 例外，它只能重编码）。

use super::super::widgets::{note, option_chip, section_title, static_id, stepper_button};
use super::super::*;
use crate::clip::{
    DEFAULT_BITRATE_KBPS, EncoderMode, PixelFormat, Quality, RateControl, VideoCodec,
};

impl PlayerApp {
    pub(super) fn render_clip_encoding(
        &self,
        dialog: &ClipDialog,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let options = &dialog.options;
        // 流复制没有解码后的样本可以加工，这一整块不出现，比置灰更省地方，
        // 也不会让人以为「勾上这些就能在无损模式下用」。
        if !options.filters_available() {
            return None;
        }

        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(section_title("编码与画质"))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .children(VideoCodec::ALL.map(|codec| {
                            option_chip(
                                static_id("clip-codec", codec.label()),
                                codec.label(),
                                options.codec == codec,
                                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                    this.update_clip_options(|options| options.codec = codec, cx);
                                }),
                            )
                        })),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .children(EncoderMode::ALL.map(|encoder| {
                            option_chip(
                                static_id("clip-encoder", encoder.label()),
                                encoder.label(),
                                options.encoder_mode == encoder,
                                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                    this.update_clip_options(
                                        |options| options.encoder_mode = encoder,
                                        cx,
                                    );
                                }),
                            )
                        })),
                )
                .child(note(options.encoder_mode.hint()))
                .child(section_title("码率控制"))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .children(RateControl::ALL.map(|control| {
                            option_chip(
                                static_id("clip-rate", control.label()),
                                control.label(),
                                options.rate_control == control,
                                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                    this.update_clip_options(
                                        |options| options.rate_control = control,
                                        cx,
                                    );
                                }),
                            )
                        })),
                )
                .child(note(options.rate_control.hint()))
                // 两种码控的控件完全不同：质量优先调档位与 CRF，固定码率只调码率。
                .when(options.rate_control == RateControl::Quality, |this| {
                    this.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .children(Quality::ALL.map(|quality| {
                                option_chip(
                                    static_id("clip-quality", quality.label()),
                                    quality.label(),
                                    options.quality == quality,
                                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                        this.update_clip_options(
                                            |options| options.quality = quality,
                                            cx,
                                        );
                                    }),
                                )
                            })),
                    )
                    .child(crf_row(options.crf, cx))
                })
                .when(options.rate_control == RateControl::Bitrate, |this| {
                    this.child(bitrate_row(options.bitrate_kbps, cx))
                })
                .child(section_title("像素格式"))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .children(PixelFormat::ALL.map(|format| {
                            option_chip(
                                static_id("clip-pixfmt", format.label()),
                                format.label(),
                                options.pixel_format == format,
                                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                    this.update_clip_options(
                                        |options| options.pixel_format = format,
                                        cx,
                                    );
                                }),
                            )
                        })),
                )
                .child(note(options.pixel_format.hint()))
                .into_any_element(),
        )
    }
}

/// 自定义 CRF 一行：`−  CRF 20  +`，点数字回到「自动」。
///
/// 只在软件编码时有意义——硬件编码器不认 `-crf`；界面按「编码器=自动」时可能
/// 落到硬件上，所以这行的说明里带一句提醒。
fn crf_row(crf: Option<u8>, cx: &mut Context<PlayerApp>) -> AnyElement {
    let label = match crf {
        Some(value) => format!("CRF {value}"),
        None => "CRF 自动".to_string(),
    };
    let step = |delta: i32| {
        cx.listener(move |this, _: &ClickEvent, _window, cx| {
            this.update_clip_options(
                |options| {
                    let current = options.crf.unwrap_or_else(|| options.quality.crf()) as i32;
                    options.crf = Some((current + delta).clamp(0, 51) as u8);
                },
                cx,
            );
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
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(text())
                        .child("自定义 CRF"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(stepper_button("clip-crf-less", "-", step(-1)))
                        .child(
                            div()
                                .id("clip-crf-value")
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
                                .child(label)
                                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    cx.stop_propagation();
                                    this.update_clip_options(|options| options.crf = None, cx);
                                })),
                        )
                        .child(stepper_button("clip-crf-more", "+", step(1))),
                ),
        )
        .child(note(
            "越小越清晰、体积越大；「自动」等于按上面的画质档换算。只对软件编码生效",
        ))
        .into_any_element()
}

/// 固定码率一行：`−  6000 kbps  +`，点数字回到默认值。
fn bitrate_row(kbps: u32, cx: &mut Context<PlayerApp>) -> AnyElement {
    let step = |delta: i32| {
        cx.listener(move |this, _: &ClickEvent, _window, cx| {
            this.update_clip_options(
                |options| {
                    let next = options.bitrate_kbps as i32 + delta;
                    options.bitrate_kbps = next.clamp(200, 50_000) as u32;
                },
                cx,
            );
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
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(text())
                        .child("目标码率"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(stepper_button("clip-bitrate-less", "-", step(-500)))
                        .child(
                            div()
                                .id("clip-bitrate-value")
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(84.0))
                                .h(px(24.0))
                                .rounded(px(6.0))
                                .bg(ui::surface_active())
                                .text_size(px(11.0))
                                .text_color(ui::accent())
                                .cursor(CursorStyle::PointingHand)
                                .hover(|style| style.bg(ui::surface_hover()))
                                .child(format!("{kbps} kbps"))
                                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    cx.stop_propagation();
                                    this.update_clip_options(
                                        |options| {
                                            options.bitrate_kbps = DEFAULT_BITRATE_KBPS;
                                        },
                                        cx,
                                    );
                                })),
                        )
                        .child(stepper_button("clip-bitrate-more", "+", step(500))),
                ),
        )
        .child(note(
            "按 500 kbps 步进；1080p 一般 5000–8000，720p 一般 2500–4000",
        ))
        .into_any_element()
}
