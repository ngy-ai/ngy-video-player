//! 裁切面板里的高级导出选项：剪切方式、编码、画质、画面变换与轨道保留。
//!
//! 选项全部收在面板里而不是设置窗口：它们只对这一次导出有效，不该变成跨会话的
//! 偏好。用户选完、点「开始」、关掉面板，下一次回到默认值。
//!
//! 音频那一段在 `audio_options.rs`：那边的控件是步进器，这里的是一排小按钮。

use super::widgets::{
    chip_row, note, note_owned, option_chip, section_title, static_id, stepper_button, toggle_row,
};
use super::*;
use crate::clip::{
    Crop, Edge, EncoderMode, ExportMode, ExportOptions, KeyframeAlign, OutputFormat, Pad,
    PixelFormat, Quality, Rotate, Scale, VideoCodec,
};

impl PlayerApp {
    /// 改一项导出参数。面板开着才生效，改完立刻重绘。
    pub(super) fn update_clip_options(
        &mut self,
        update: impl FnOnce(&mut ExportOptions),
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.clip.as_mut() else {
            return;
        };
        update(&mut dialog.options);
        cx.notify();
    }

    pub(super) fn render_clip_options(
        &self,
        dialog: &ClipDialog,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let options = &dialog.options;
        // 流复制没有解码后的样本可以加工，滤镜与响度整块不出现，比置灰更省地方，
        // 也不会让人以为「勾上这些就能在无损模式下用」。
        let exact = options.filters_available();
        // GIF 只能重编码，导出方式没有可选性；露出来只会让人以为能无损导出。
        let gif = options.format == OutputFormat::Gif;

        let mut body = div().flex().flex_col().gap(px(8.0));
        if !gif {
            body = body
                .child(section_title("导出方式"))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .children(ExportMode::ALL.map(|mode| {
                            option_chip(
                                static_id("clip-mode", mode.label()),
                                mode.label(),
                                options.mode == mode,
                                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                    this.update_clip_options(|options| options.mode = mode, cx);
                                }),
                            )
                        })),
                )
                .child(note(options.mode.hint()));
        }

        body = body
            .child(section_title("输出"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .children(OutputFormat::ALL.map(|format| {
                        option_chip(
                            static_id("clip-format", format.label()),
                            format.label(),
                            options.format == format,
                            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                this.update_clip_options(|options| options.format = format, cx);
                            }),
                        )
                    })),
            )
            .child(note(options.format.hint()));

        // 流复制只能在关键帧上起步，用户给的秒数几乎总落在两个关键帧之间，
        // 于是必然要选一边吃亏。其余模式是帧精确的，不需要这个选择。
        if options.mode == ExportMode::Copy && !gif {
            body =
                body.child(section_title("关键帧对齐"))
                    .child(div().flex().items_center().gap(px(6.0)).children(
                        KeyframeAlign::ALL.map(|align| {
                            option_chip(
                                static_id("clip-align", align.label()),
                                align.label(),
                                options.keyframe_align == align,
                                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                    this.update_clip_options(
                                        |options| options.keyframe_align = align,
                                        cx,
                                    );
                                }),
                            )
                        }),
                    ))
                    .child(note(options.keyframe_align.hint()));
        }

        if exact {
            body = body
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
                .child(
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
                .child(self.render_clip_audio_options(options, cx));
        }

        body = body
            .child(section_title("时间戳"))
            .child(toggle_row(
                "clip-option-fix-pts",
                "修复缺失/负时间戳",
                options.fix_timestamps,
                |this, cx| {
                    this.update_clip_options(|options| options.fix_timestamps ^= true, cx);
                },
                cx,
            ))
            .child(note("对流复制也生效；音频漂移校正只在重编码时可用。"))
            .child(section_title("轨道"));

        // GIF 没有音轨可留，这个开关留着只会让人以为能导出带声音的动图。
        if !gif {
            body = body.child(toggle_row(
                "clip-option-audio",
                "保留音轨",
                options.keep_audio,
                |this, cx| {
                    this.update_clip_options(|options| options.keep_audio ^= true, cx);
                },
                cx,
            ));
        }

        // 烧录只对文本字幕有意义；这个文件没有可烧的轨道时，这一行不出现。
        if let Some(subtitle) = &dialog.subtitle {
            body = body
                .child(toggle_row(
                    "clip-option-burn-subtitle",
                    "烧录内置字幕（硬字幕）",
                    dialog.burn_subtitle,
                    |this, cx| this.toggle_clip_burn(cx),
                    cx,
                ))
                .child(note_owned(format!(
                    "把「{}」画进画面，烧进去就再也关不掉；原字幕轨不再复制",
                    subtitle.name
                )));
        }

        if options.format == OutputFormat::Mkv {
            // 烧录之后软字幕不再复制，这个开关留着只会让人以为还能同时保留两份。
            if !dialog.burn_subtitle {
                body = body.child(toggle_row(
                    "clip-option-subtitles",
                    "保留字幕",
                    options.keep_subtitles,
                    |this, cx| {
                        this.update_clip_options(|options| options.keep_subtitles ^= true, cx);
                    },
                    cx,
                ));
            }
            body = body.child(toggle_row(
                "clip-option-chapters",
                "章节与元数据",
                options.keep_chapters,
                |this, cx| {
                    this.update_clip_options(|options| options.keep_chapters ^= true, cx);
                },
                cx,
            ));
        } else if gif {
            body = body.child(note("GIF 只包含画面：没有声音，也没有字幕与章节。"));
        } else {
            body = body.child(note("HLS 只保留画面与音轨，字幕与章节不写入。"));
        }

        body.into_any_element()
    }

    fn nudge_crop(&mut self, edge: Edge, delta: f32, cx: &mut Context<Self>) {
        self.update_clip_options(|options| options.transform.crop.nudge(edge, delta), cx);
    }
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
