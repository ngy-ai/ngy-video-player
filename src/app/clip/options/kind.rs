//! 剪切方式与输出格式：这三组选项是下面所有设置的前提。
//!
//! 「保留区间 / 删除中间段 / 在切点分割」决定产物是一个还是两个、能不能流复制；
//! 输出格式决定音频与字幕还能不能保留。排在面板最上面。
//!
//! 批处理（文件夹来源）只做整段导出，这一块由调用方判断后整块收掉。

use super::super::widgets::{note, option_chip, section_title, static_id, toggle_row};
use super::super::*;
use super::controls::{StepperTarget, step_row};
use crate::clip::{CutKind, ExportMode, KeyframeAlign, OutputFormat};

impl PlayerApp {
    /// 区间在三种操作里的含义不同：保留它、抠掉它、或者拿它的起点当切点。
    pub(super) fn render_clip_kind(
        &self,
        dialog: &ClipDialog,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let kind = dialog.kind;
        Some(
            div()
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
                }))
                .into_any_element(),
        )
    }

    /// 导出方式（精确重编码 / 无损流复制）、输出格式与关键帧对齐。
    pub(super) fn render_clip_format(
        &self,
        dialog: &ClipDialog,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let options = &dialog.options;
        // GIF 只能重编码，导出方式没有可选性；露出来只会让人以为能无损导出。
        let gif = options.format == OutputFormat::Gif;
        // 删除中间段只能是 mkv：拼接要复制流，HLS 分片与 GIF 都装不下两段拼起来的流。
        let cut_out = dialog.kind != CutKind::Keep;

        let mut body = div().flex().flex_col().gap(px(8.0));
        let mut any = false;

        if !gif && !cut_out {
            any = true;
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

        if !cut_out {
            any = true;
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
                .child(note(options.format.hint()))
                .when(options.format.is_split(), |this| {
                    this.child(step_row(
                        "clip-split-size",
                        "每段大小",
                        format!("{} MB", options.split_mb),
                        "切点只能落在关键帧上，每段实际会略小；700MB 是一张 DVD 的容量。",
                        StepperTarget::SplitSize,
                        cx,
                    ))
                });
        }

        // 流复制只能在关键帧上起步，用户给的秒数几乎总落在两个关键帧之间，
        // 于是必然要选一边吃亏。其余模式是帧精确的，不需要这个选择。
        if options.mode == ExportMode::Copy && !gif {
            any = true;
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

        any.then(|| body.into_any_element())
    }
}
