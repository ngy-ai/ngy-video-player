//! 裁切面板里的音频选项：响度、增益、偏移、漂移校正、采样率与声道。
//!
//! 和画面选项分成两个文件：音频这边一条条都是「步进器 + 说明」，画面那边是
//! 「一排小按钮 + 四边裁切格」，排版逻辑完全不同，混在一处只会让两边都难读。

use super::widgets::{chip_row, note, section_title, toggle_row};
use super::*;
use crate::clip::{ExportOptions, OutputFormat};

/// 采样率档位：跟随源 + 三种最常见的值。
///
/// 16 kHz 不是随便挑的：语音笔记、有声书、老式车载机都停在这一档，转过去体积
/// 能掉一半以上。
const SAMPLE_RATES: [Option<u32>; 4] = [None, Some(48_000), Some(44_100), Some(16_000)];

/// 声道档位：跟随源 / 立体声 / 单声道。
const CHANNELS: [Option<u8>; 3] = [None, Some(2), Some(1)];

fn sample_rate_label(rate: Option<u32>) -> &'static str {
    match rate {
        None => "源采样率",
        Some(48_000) => "48 kHz",
        Some(44_100) => "44.1 kHz",
        Some(16_000) => "16 kHz",
        // 数组里只有上面几种；这里给一个不会误导的兜底。
        Some(_) => "自定义",
    }
}

fn channel_label(channels: Option<u8>) -> &'static str {
    match channels {
        None => "源声道",
        Some(1) => "单声道",
        Some(2) => "立体声",
        Some(_) => "多声道",
    }
}

impl PlayerApp {
    /// 音频那一段。返回的是一个整块，直接接到面板的 body 上。
    pub(super) fn render_clip_audio_options(
        &self,
        options: &ExportOptions,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // GIF 没有音轨，这些开关一条都用不上。
        if options.format == OutputFormat::Gif {
            return div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(section_title("音频"))
                .child(note("GIF 没有音轨，音频选项不适用。"))
                .into_any_element();
        }

        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
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

        // 直接复制时下面这些没有任何作用：样本根本不解出来。收起来比置灰更
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
                // 两遍扫描只在开了标准化之后才有意义：单独摆一行会让人以为它
                // 自己就能调音量。
                .when(options.normalize_audio, |this| {
                    this.child(toggle_row(
                        "clip-option-loudnorm-two-pass",
                        "两遍扫描（不压动态）",
                        options.two_pass_loudness,
                        |this, cx| {
                            this.update_clip_options(
                                |options| options.two_pass_loudness ^= true,
                                cx,
                            );
                        },
                        cx,
                    ))
                    .child(note("先量一遍整段再统一增益；多解一遍音频，长片会慢一些"))
                })
                .child(toggle_row(
                    "clip-option-sync-audio",
                    "音频漂移校正（长录制）",
                    options.sync_audio,
                    |this, cx| {
                        this.update_clip_options(|options| options.sync_audio ^= true, cx);
                    },
                    cx,
                ))
                .child(volume_row(options.volume_db, cx))
                .child(audio_offset_row(options.audio_offset, cx))
                // 采样率与声道只在真正重编码音频时才有意义：复制码流时这些
                // 参数写进去会被 ffmpeg 忽略，露出来只会让人以为没生效。
                .child(chip_row(
                    "clip-audio-rate",
                    &SAMPLE_RATES,
                    options.sample_rate,
                    sample_rate_label,
                    |options, rate| options.sample_rate = rate,
                    cx,
                ))
                .child(chip_row(
                    "clip-audio-channels",
                    &CHANNELS,
                    options.channels,
                    channel_label,
                    |options, channels| options.channels = channels,
                    cx,
                ))
                .child(note(
                    "采样率/声道跟随源时不再额外传参；单声道适合对白与语音笔记。",
                ));
        }

        body.into_any_element()
    }
}

/// 音量增益一行：`-1dB` / 当前值 / `+1dB`，点数字归零。
///
/// 同样用步进而不是滑块：判断「差 1dB」只需要耳朵，拖滑块反而找不到。
fn volume_row(db: f32, cx: &mut Context<PlayerApp>) -> AnyElement {
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
                        .child("音量增益"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(settings_button(
                            "clip-volume-back",
                            "-1dB",
                            true,
                            cx.listener(|this, _: &ClickEvent, _window, cx| {
                                this.nudge_volume(-1.0, cx);
                            }),
                        ))
                        .child(
                            div()
                                .id("clip-volume-value")
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(58.0))
                                .h(px(32.0))
                                .rounded(px(6.0))
                                .bg(ui::surface_hover())
                                .text_size(px(11.0))
                                .text_color(ui::accent())
                                .cursor(CursorStyle::PointingHand)
                                .hover(|style| style.bg(ui::surface_active()))
                                .child(format!("{db:+.0}dB"))
                                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    cx.stop_propagation();
                                    this.update_clip_options(|options| options.volume_db = 0.0, cx);
                                })),
                        )
                        .child(settings_button(
                            "clip-volume-forward",
                            "+1dB",
                            true,
                            cx.listener(|this, _: &ClickEvent, _window, cx| {
                                this.nudge_volume(1.0, cx);
                            }),
                        )),
                ),
        )
        .child(note("正数变大、负数变小；点数值归零。作用在响度标准化之后"))
        .into_any_element()
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
    fn nudge_volume(&mut self, delta: f32, cx: &mut Context<Self>) {
        self.update_clip_options(
            |options| {
                // ±20dB 之外不是「增益」而是「失真」，超出范围直接夹住。
                options.volume_db = (options.volume_db + delta).clamp(-20.0, 20.0);
            },
            cx,
        );
    }

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
