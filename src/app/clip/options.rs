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
    ColorMatrix, ColorRange, Crop, CutKind, DEFAULT_BITRATE_KBPS, Edge, EncoderMode, ExportMode,
    ExportOptions, FrameRate, KeyframeAlign, MAX_FREEZE_SECONDS, OutputFormat, Pad, PixelFormat,
    Quality, RateControl, Rotate, Scale, VideoCodec, Watermark, WatermarkPosition,
};
use gpui_kit::component::input::Input;

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

        // 区间在三种操作里的含义不同：保留它、抠掉它、或者拿它的起点当切点。
        // 这三个开关放在最上面，因为它们改变的是下面所有设置的前提。
        let kind = dialog.kind;
        let cut_out = kind != CutKind::Keep;

        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(toggle_row(
                "clip-option-remove-middle",
                "删除这段（保留前后）",
                kind == CutKind::RemoveMiddle,
                |this, cx| this.toggle_clip_kind(CutKind::RemoveMiddle, cx),
                cx,
            ))
            .child(toggle_row(
                "clip-option-split",
                "在「开始」处分成两个文件",
                kind == CutKind::Split,
                |this, cx| this.toggle_clip_kind(CutKind::Split, cx),
                cx,
            ))
            .child(note(match kind {
                CutKind::Keep => "默认只保留选中区间；上面两项都不勾时，结束为空表示一直到片尾",
                CutKind::RemoveMiddle => {
                    "把选中的区间抠掉，首尾两段拼成一个文件；必须重编码，产物固定为 mkv"
                }
                CutKind::Split => "在「开始」处切开，写出段落1 与段落2 两个文件；必须重编码",
            }));

        if !gif && !cut_out {
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

        // 删除中间段只能是 mkv：拼接要复制流，HLS 分片与 GIF 都装不下两段拼起来的流。
        if !cut_out {
            body = body
                .child(section_title("输出"))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
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
        }

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
            body = body.child(note("切片输出只保留画面与音轨，字幕与章节不写入。"));
        }

        body.into_any_element()
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

/// 三处步进器共用一套加减与归位逻辑。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StepperTarget {
    /// 开头定格，0.5 秒一格。
    Freeze,
    /// 水印字号，4px 一格。
    FontSize,
    /// 水印不透明度，10% 一格。
    Opacity,
}

fn apply_step(options: &mut ExportOptions, target: StepperTarget, delta: i32) {
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
    }
}

fn reset_value(options: &mut ExportOptions, target: StepperTarget) {
    match target {
        StepperTarget::Freeze => options.freeze_lead = 0.0,
        StepperTarget::FontSize => options.watermark.size = Watermark::default().size,
        StepperTarget::Opacity => options.watermark.opacity = Watermark::default().opacity,
    }
}

/// 一行步进器：`−  当前值  +`，点数值回到默认。
///
/// 三个控件长得一样、只有步长与量程不同，做成一个函数，免得改了字号那行
/// 忘了改不透明度那行。
fn step_row(
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
