//! 裁切面板里的高级导出选项：剪切方式、编码、画质、滤镜与轨道保留。
//!
//! 选项全部收在面板里而不是设置窗口：它们只对这一次导出有效，不该变成跨会话的
//! 偏好。用户选完、点「开始」、关掉面板，下一次回到默认值。

use super::*;
use crate::clip::{ExportMode, ExportOptions, Quality, VideoCodec};

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
            .child(note(options.mode.hint()));

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
                .child(section_title("音频"))
                .child(toggle_row(
                    "clip-option-loudnorm",
                    "响度标准化（-16 LUFS）",
                    options.normalize_audio,
                    |this, cx| {
                        this.update_clip_options(|options| options.normalize_audio ^= true, cx);
                    },
                    cx,
                ));
        }

        body.child(section_title("轨道"))
            .child(toggle_row(
                "clip-option-audio",
                "保留音轨",
                options.keep_audio,
                |this, cx| {
                    this.update_clip_options(|options| options.keep_audio ^= true, cx);
                },
                cx,
            ))
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
            ))
            .into_any_element()
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
