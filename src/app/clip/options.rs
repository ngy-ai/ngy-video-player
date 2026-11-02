//! 裁切面板里的高级导出选项：剪切方式、编码、画质、滤镜与轨道保留。
//!
//! 选项全部收在面板里而不是设置窗口：它们只对这一次导出有效，不该变成跨会话的
//! 偏好。用户选完、点「开始」、关掉面板，下一次回到默认值。

use super::*;
use crate::clip::{
    EncoderMode, ExportMode, ExportOptions, KeyframeAlign, OutputFormat, PixelFormat, Quality,
    VideoCodec,
};

impl PlayerApp {
    /// 改一项导出参数。面板开着才生效，改完立刻重绘。
    fn update_clip_options(
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

        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
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
            .child(note(options.mode.hint()))
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
        if options.mode == ExportMode::Copy {
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
                .child(section_title("音频"));

            // HLS 的 TS 分片装不下 FLAC / Opus，那条路上复制音频必然失败，
            // 所以干脆不提供这个开关。
            if options.format == OutputFormat::Mkv {
                body = body
                    .child(toggle_row(
                        "clip-option-audio-copy",
                        "直接复制音频（不重编码）",
                        options.audio_copy,
                        |this, cx| {
                            this.update_clip_options(|options| options.audio_copy ^= true, cx);
                        },
                        cx,
                    ))
                    .child(note(
                        "保住 DTS-HD / TrueHD 等原始音轨；此时响度、偏移与漂移校正都不生效。",
                    ));
            }

            // 直接复制时下面三行没有任何作用：样本根本不解出来。收起来比置灰更
            // 不容易让人以为「勾了只是暂时没生效」。
            if options.audio_filters_available() {
                body = body
                    .child(toggle_row(
                        "clip-option-loudnorm",
                        "响度标准化（-16 LUFS）",
                        options.normalize_audio,
                        |this, cx| {
                            this.update_clip_options(|options| options.normalize_audio ^= true, cx);
                        },
                        cx,
                    ))
                    .child(toggle_row(
                        "clip-option-sync-audio",
                        "音频漂移校正（长录制）",
                        options.sync_audio,
                        |this, cx| {
                            this.update_clip_options(|options| options.sync_audio ^= true, cx);
                        },
                        cx,
                    ))
                    .child(audio_offset_row(options.audio_offset, cx));
            }
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
            .child(section_title("轨道"))
            .child(toggle_row(
                "clip-option-audio",
                "保留音轨",
                options.keep_audio,
                |this, cx| {
                    this.update_clip_options(|options| options.keep_audio ^= true, cx);
                },
                cx,
            ));

        if options.format == OutputFormat::Mkv {
            body = body
                .child(toggle_row(
                    "clip-option-subtitles",
                    "保留字幕",
                    options.keep_subtitles,
                    |this, cx| {
                        this.update_clip_options(|options| options.keep_subtitles ^= true, cx);
                    },
                    cx,
                ))
                .child(toggle_row(
                    "clip-option-chapters",
                    "章节与元数据",
                    options.keep_chapters,
                    |this, cx| {
                        this.update_clip_options(|options| options.keep_chapters ^= true, cx);
                    },
                    cx,
                ));
        } else {
            body = body.child(note("HLS 只保留画面与音轨，字幕与章节不写入。"));
        }

        body.into_any_element()
    }
}

fn section_title(label: &'static str) -> AnyElement {
    div()
        .pt(px(2.0))
        .text_size(px(11.0))
        .text_color(text_muted())
        .child(label)
        .into_any_element()
}

fn note(text: &'static str) -> AnyElement {
    div()
        .text_size(px(10.0))
        .text_color(text_faint())
        .whitespace_normal()
        .child(text)
        .into_any_element()
}

/// 一组互斥选项里的一颗小按钮。选中项用强调色填充。
fn option_chip(
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

/// 复选框一行。`update` 只负责翻转字段，重绘与取值由调用方统一处理。
fn toggle_row(
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
fn static_id(prefix: &'static str, label: &'static str) -> SharedString {
    SharedString::from(format!("{prefix}-{label}"))
}

/// 音频偏移一行：`-0.1s` / 当前值 / `+0.1s`，点数字归零。
///
/// 用步进按钮而不是滑块：±5 秒的量程里，滑块拖到 0.1 秒精度几乎不可能，
/// 而修音画不同步恰恰是 0.1 秒一级的调整。
fn audio_offset_row(offset: f64, cx: &mut Context<PlayerApp>) -> AnyElement {
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
                        .child("音频偏移"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(settings_button(
                            "clip-offset-back",
                            "-0.1s",
                            true,
                            cx.listener(|this, _: &ClickEvent, _window, cx| {
                                this.nudge_audio_offset(-0.1, cx);
                            }),
                        ))
                        .child(
                            div()
                                .id("clip-offset-value")
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(52.0))
                                .h(px(32.0))
                                .rounded(px(6.0))
                                .bg(ui::surface_hover())
                                .text_size(px(11.0))
                                .text_color(ui::accent())
                                .cursor(CursorStyle::PointingHand)
                                .hover(|style| style.bg(ui::surface_active()))
                                .child(format!("{offset:+.1}s"))
                                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    cx.stop_propagation();
                                    this.update_clip_options(
                                        |options| options.audio_offset = 0.0,
                                        cx,
                                    );
                                })),
                        )
                        .child(settings_button(
                            "clip-offset-forward",
                            "+0.1s",
                            true,
                            cx.listener(|this, _: &ClickEvent, _window, cx| {
                                this.nudge_audio_offset(0.1, cx);
                            }),
                        )),
                ),
        )
        .child(note("正数=声音延后，负数=提前；点数值归零"))
        .into_any_element()
}

impl PlayerApp {
    fn nudge_audio_offset(&mut self, delta: f64, cx: &mut Context<Self>) {
        self.update_clip_options(
            |options| {
                // 一次加 0.1 秒，浮点累加会跑出 0.30000000000000004 这类值；
                // 每步取整到一位小数，显示和传参都稳定。
                let next = ((options.audio_offset + delta) * 10.0).round() / 10.0;
                options.audio_offset = next.clamp(-5.0, 5.0);
            },
            cx,
        );
    }
}
