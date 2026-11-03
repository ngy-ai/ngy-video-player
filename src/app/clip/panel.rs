//! 裁切面板的绘制：编辑态、运行态、完成态。
//!
//! 与状态、任务分开成文件：面板要跟着「右侧通道只有一条」的布局约束走，
//! 改动频率和后台任务不同，混在一起时一次改样式就要翻过整段 ffmpeg 逻辑。

use super::*;
use crate::clip as clip_core;
use std::path::PathBuf;

impl PlayerApp {
    /// 右侧面板本体。与播放列表共用一条通道：视频和控制条按它让位，
    /// 所以面板不会压住进度条，用户可以直接用主进度条选点。
    pub(crate) fn render_clip_panel(
        &self,
        top: Pixels,
        width: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.clip.as_ref()?;
        let body = match &dialog.state {
            ClipState::Editing | ClipState::Failed(_) => self.render_clip_editor(dialog, cx),
            ClipState::Running { progress } => self.render_clip_progress(*progress, cx),
            ClipState::Done(paths) => self.render_clip_done(paths, cx),
        };

        Some(
            div()
                .id("clip-panel")
                .absolute()
                .top(top)
                .right_0()
                .bottom_0()
                .w(px(width))
                .flex()
                .flex_col()
                .bg(gpui_kit::rgba(0x131317e6))
                .border_l_1()
                .border_color(gpui_kit::rgb(0x26262e))
                // 面板上的点击不该漏到画面去（画面那一层负责播放/暂停）。
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                    if this.item_menu.take().is_some() {
                        cx.notify();
                    }
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .pl(px(14.0))
                        .pr(px(6.0))
                        .py(px(8.0))
                        .child(
                            div()
                                .text_size(px(14.0))
                                .text_color(text())
                                .child("裁切片段"),
                        )
                        .child(icon_button(
                            "clip-close",
                            IconName::X,
                            false,
                            false,
                            cx.listener(|this, _: &ClickEvent, _window, cx| {
                                this.close_clip_dialog(cx);
                            }),
                        )),
                )
                .child(
                    div()
                        .id("clip-panel-body")
                        .flex_1()
                        .min_h(px(0.0))
                        // 窗口矮时内容可滚动，按钮不会掉到面板外面。
                        .overflow_y_scroll()
                        .px(px(14.0))
                        .pb(px(14.0))
                        .child(body),
                )
                .into_any_element(),
        )
    }

    fn render_clip_editor(&self, dialog: &ClipDialog, cx: &mut Context<Self>) -> AnyElement {
        let name = file_name(&dialog.path);
        let range = dialog.range();
        // 两端都留空时按钮不可用：这不是「复制整份文件」的入口。
        let can_start = matches!(range, Ok(Some(_)));
        let hint: Option<(String, bool)> = match &range {
            Ok(None) => Some(("开始和结束都留空：请至少填写一端".to_string(), false)),
            Ok(Some(range)) => Some((
                format!(
                    "{} → {}，共 {}",
                    clip_core::format_time(range.start),
                    clip_core::format_time(range.end),
                    clip_core::format_time(range.duration())
                ),
                true,
            )),
            Err(error) => Some((error.message().to_string(), false)),
        };

        // 当前进度用和「全长」同一套格式：拖完进度条、逐帧微调以后，眼睛要能
        // 立刻核对停在哪一帧，控制条上的 mm:ss 精度不够。
        let position = self.snapshot.position.clamp(0.0, dialog.duration.max(0.0));

        let mut body = div().flex().flex_col().gap(px(12.0)).child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(text())
                        .child(ui::ellipsis(name)),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(text_muted())
                        .child(format!("全长 {}", clip_core::format_time(dialog.duration))),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(text_muted())
                        .child(format!("当前 {}", clip_core::format_time(position))),
                ),
        );

        // 失败原因来自 ffmpeg，可能有多行；单独一块红字，别和输入校验混在一起。
        if let ClipState::Failed(message) = &dialog.state {
            body = body.child(
                div()
                    .p(px(10.0))
                    .rounded(px(8.0))
                    .bg(gpui_kit::rgba(0x2a1414f2))
                    .text_size(px(11.0))
                    .text_color(ui::danger())
                    // ffmpeg 的错误是多行的，别挤成一行。
                    .whitespace_normal()
                    .child(message.clone()),
            );
        }

        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .child(self.render_clip_field(
                    ClipField::Start,
                    "开始",
                    &dialog.start,
                    "留空 = 从头；也收 01:02:03;15",
                    cx,
                ))
                .child(self.render_clip_field(
                    ClipField::End,
                    "结束",
                    &dialog.end,
                    "留空 = 到最后；也收 01:02:03;15",
                    cx,
                )),
        );

        if let Some((text, ok)) = hint {
            body = body.child(
                div()
                    .text_size(px(11.0))
                    .text_color(if ok { ui::accent() } else { ui::danger() })
                    .child(text),
            );
        }

        body.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(settings_button(
                    "clip-mark-start",
                    "设为开始",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.clip_mark(ClipField::Start, cx);
                    }),
                ))
                .child(settings_button(
                    "clip-mark-end",
                    "设为结束",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.clip_mark(ClipField::End, cx);
                    }),
                )),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(settings_button(
                    "clip-frame-back",
                    "上一帧",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.step_frame(-1, cx);
                    }),
                ))
                .child(settings_button(
                    "clip-toggle-play",
                    if self.snapshot.paused {
                        "播放"
                    } else {
                        "暂停"
                    },
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.toggle_play(cx);
                    }),
                ))
                .child(settings_button(
                    "clip-frame-step",
                    "下一帧",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.step_frame(1, cx);
                    }),
                )),
        )
        .child(self.render_clip_options(dialog, cx))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(settings_button(
                    "clip-preview-range",
                    "预览片段",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.clip_preview_range(cx);
                    }),
                ))
                .child(primary_button(
                    "clip-start",
                    if dialog.kind == clip_core::CutKind::RemoveMiddle {
                        "删除中间段"
                    } else if dialog.kind == clip_core::CutKind::Split {
                        "分割为两个文件"
                    } else if dialog.options.format.is_segmented() {
                        "开始导出切片"
                    } else if dialog.options.format == clip_core::OutputFormat::Gif {
                        "导出 GIF"
                    } else if dialog.options.mode == clip_core::ExportMode::Copy {
                        "开始无损剪切"
                    } else {
                        "开始裁切"
                    },
                    can_start,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.start_clip(cx);
                    }),
                )),
        )
        .child(
            div()
                .text_size(px(10.0))
                .text_color(text_muted())
                .whitespace_normal()
                .child(
                    "用控制条上的进度条定位，停在哪一帧就点「设为开始 / 设为结束」。\
                     空格播放/暂停，←/→ 5 秒，Ctrl/⌘+←/→ 逐帧。",
                ),
        )
        .child(
            div()
                .text_size(px(10.0))
                .text_color(text_faint())
                .whitespace_normal()
                .child(if dialog.kind == clip_core::CutKind::RemoveMiddle {
                    "把选中的区间抠掉后重新编码拼接；中途要占一份片长的临时空间。"
                } else if dialog.kind == clip_core::CutKind::Split {
                    "在「开始」处切成前后两个文件，都落在原片同目录；结束时间在这里不参与。"
                } else if dialog.options.format.is_segmented() {
                    "输出播放列表与同名分片；只保留画面与音轨，字幕与章节不写入。"
                } else if dialog.options.format == clip_core::OutputFormat::Gif {
                    "导出 12fps、480 宽的循环动图；没有声音。片段越长，文件越大。"
                } else if dialog.options.mode == clip_core::ExportMode::Copy {
                    "流复制为 mkv：不重新编码，几秒完成；切点会吸附到关键帧上，\
                     实际起点与设定值可能差一小段，导出完成后会告诉你差多少。"
                } else {
                    "重编码为 mkv，耗时与片长相关；开始后可取消。"
                }),
        )
        .into_any_element()
    }

    fn render_clip_field(
        &self,
        field: ClipField,
        label: &'static str,
        value: &str,
        hint: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self
            .clip
            .as_ref()
            .is_some_and(|dialog| dialog.field == Some(field));
        let id = match field {
            ClipField::Start => "clip-start-input",
            ClipField::End => "clip-end-input",
        };
        let value = value.to_string();

        div()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .flex_col()
            .gap(px(5.0))
            // 提示放在标题行右侧，不再塞进输入框：框里留着占位文字时，光标一进去
            // 就成了「留空 = 从头|」，看着像已经输入了内容，又退不掉。
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text_muted())
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(text_faint())
                            .child(hint),
                    ),
            )
            .child(
                div()
                    .id(id)
                    .h(px(34.0))
                    .flex()
                    .items_center()
                    .px(px(10.0))
                    .rounded(px(8.0))
                    .bg(ui::surface_active())
                    .border_1()
                    .border_color(if active { ui::accent() } else { ui::track() })
                    .cursor(CursorStyle::IBeam)
                    // 用 mouse_down 而不是 click：根节点在同一次按下时会清焦点，
                    // 这里先截断冒泡，焦点才落得住。
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                            cx.stop_propagation();
                            if let Some(dialog) = this.clip.as_mut() {
                                dialog.field = Some(field);
                            }
                            cx.notify();
                        }),
                    )
                    // 空就是空：留空的含义已经在标题行，框里只放真正输入的内容，
                    // 光标才有地方落。
                    .when(!value.is_empty(), |this| {
                        this.child(div().text_size(px(13.0)).text_color(text()).child(value))
                    })
                    // 光标固定画在末尾：这里不做左右移动与选区，输入永远是追加/退格。
                    .when(active, |this| {
                        this.child(div().ml(px(1.0)).w(px(1.0)).h(px(14.0)).bg(ui::accent()))
                    }),
            )
            .into_any_element()
    }

    fn render_clip_progress(&self, progress: f32, cx: &mut Context<Self>) -> AnyElement {
        let percent = (progress * 100.0).round().clamp(0.0, 100.0) as i32;
        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(text_muted())
                    .child(format!("正在写入片段… {percent}%")),
            )
            .child(
                div()
                    .h(px(6.0))
                    .w_full()
                    .rounded_full()
                    .bg(ui::track())
                    .child(
                        div()
                            .h_full()
                            .rounded_full()
                            .bg(ui::accent())
                            .w(gpui_kit::relative(progress.clamp(0.0, 1.0))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .child(settings_button(
                        "clip-abort",
                        "取消",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            // 关闭即取消：任务线程负责杀掉 ffmpeg 并删掉半成品。
                            this.close_clip_dialog(cx);
                        }),
                    )),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(text_faint())
                    .child("裁切在后台进行，关掉面板会取消并删除半成品。"),
            )
            .into_any_element()
    }

    fn render_clip_done(&self, paths: &[PathBuf], cx: &mut Context<Self>) -> AnyElement {
        let mut body = div().flex().flex_col().gap(px(10.0)).child(
            div()
                .text_size(px(13.0))
                .text_color(text())
                .child(if paths.len() > 1 {
                    format!("已保存 {} 个文件", paths.len())
                } else {
                    "片段已保存".to_string()
                }),
        );
        // 分割会写出两个文件，逐个列出来，用户才知道东西落在哪。
        for path in paths {
            body = body.child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_muted())
                    .whitespace_normal()
                    .child(path.display().to_string()),
            );
        }

        // 无损剪切只能在关键帧上起步，实际起点与设定值几乎总差一截。
        // 不说清楚的话，用户会以为剪切位置不准。
        if let Some(dialog) = self.clip.as_ref()
            && let Some(actual) = dialog.aligned
        {
            let wanted = dialog
                .range()
                .ok()
                .flatten()
                .map(|range| range.start)
                .unwrap_or(actual);
            let delta = actual - wanted;
            let offset = if delta < 0.0 {
                format!("多含 {:.2} 秒", -delta)
            } else {
                format!("少含 {:.2} 秒", delta)
            };
            body = body.child(
                div()
                    .text_size(px(11.0))
                    .text_color(ui::accent())
                    .whitespace_normal()
                    .child(format!(
                        "无损剪切只能从关键帧起步：设定 {}，实际 {} 开始，{offset}。",
                        clip_core::format_time(wanted),
                        clip_core::format_time(actual),
                    )),
            );
        }

        body.child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.0))
                .child(settings_button(
                    "clip-reveal",
                    "定位",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.clip_reveal_result(cx);
                    }),
                ))
                .child(settings_button(
                    "clip-open",
                    "打开",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.clip_open_result(cx);
                    }),
                ))
                .child(primary_button(
                    "clip-finish",
                    "完成",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.close_clip_dialog(cx);
                    }),
                )),
        )
        .into_any_element()
    }
}
