//! 转换面板的渲染：来源、目标格式、进度与结果。
//!
//! 与播放列表共用右侧通道，所以它挤掉列表、并按自己的宽度让画面与控制条让位。
//! 面板不是模态的：用户可以一边看一边挑格式，转过的那一集还能接着播。

use super::super::*;
use super::state::{ConvertDialog, ConvertSource, ConvertState, ScanState};
use crate::convert::{TargetFormat, TargetKind};

/// 格式格子一行放几个。目标格式有几十个，四列能让列表短一半；再密就装不下
/// `3gpp`、`webm` 这种四字符的扩展名了。
const COLUMNS: usize = 4;
/// 结果页最多列几条失败原因，剩下的只报个数。
const MAX_FAILURES_SHOWN: usize = 3;

impl PlayerApp {
    /// 右侧面板本体。裁切面板与播放列表让出同一条通道。
    pub(in crate::app) fn render_convert_panel(
        &self,
        top: Pixels,
        width: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.convert.as_ref()?;
        // 格式表很长，编辑态把「开始转换」钉在面板底部：滚到一半也该点得到。
        let (body, footer) = match &dialog.state {
            ConvertState::Editing | ConvertState::Failed(_) => (
                self.render_convert_editor(dialog, cx),
                Some(self.render_convert_footer(dialog, cx)),
            ),
            ConvertState::Running { .. } => (self.render_convert_running(dialog, cx), None),
            ConvertState::Done { .. } => (self.render_convert_done(dialog, cx), None),
        };

        Some(
            div()
                .id("convert-panel")
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
                                .child("格式转换"),
                        )
                        .child(icon_button(
                            "convert-close",
                            IconName::X,
                            false,
                            false,
                            cx.listener(|this, _: &ClickEvent, _window, cx| {
                                this.close_convert_dialog(cx);
                            }),
                        )),
                )
                .child(
                    div()
                        .id("convert-panel-body")
                        .flex_1()
                        .min_h(px(0.0))
                        // 格式表有几十项，窗口再高也装不下，让它自己滚。
                        .overflow_y_scroll()
                        .px(px(14.0))
                        .pb(px(14.0))
                        .child(body),
                )
                .children(footer)
                .into_any_element(),
        )
    }

    fn render_convert_editor(&self, dialog: &ConvertDialog, cx: &mut Context<Self>) -> AnyElement {
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(self.render_convert_source(dialog, cx))
            .child(self.render_convert_formats(dialog, cx));

        // 失败原因来自 ffmpeg，可能有多行；单独一块红字，别和来源说明混在一起。
        if let ConvertState::Failed(message) = &dialog.state {
            body = body.child(
                div()
                    .p(px(10.0))
                    .rounded(px(8.0))
                    .bg(gpui_kit::rgba(0x2a1414f2))
                    .text_size(px(11.0))
                    .text_color(ui::danger())
                    .whitespace_normal()
                    .child(message.clone()),
            );
        }

        body.into_any_element()
    }

    /// 面板底部的固定操作区：说明 + 开始按钮。不跟着格式表一起滚。
    fn render_convert_footer(&self, dialog: &ConvertDialog, cx: &mut Context<Self>) -> AnyElement {
        // 有文件可转才让按钮亮起来：点下去什么都没发生的按钮最让人困惑。
        let ready = dialog.editing() && self.convert_input_count() > 0;
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .px(px(14.0))
            .py(px(10.0))
            .border_t_1()
            .border_color(gpui_kit::rgb(0x26262e))
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(text_faint())
                    .whitespace_normal()
                    .child(
                        "输出保存在原文件同目录，命名如「片名 - 转换.mkv」；同名文件已存在时\
                         自动编号，不会覆盖原片。",
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .child(primary_button(
                        "convert-start",
                        "开始转换",
                        ready,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.start_convert(cx);
                        }),
                    )),
            )
            .into_any_element()
    }

    /// 来源区：当前文件 / 文件夹两个入口，加一行「这一批有多少个文件」。
    fn render_convert_source(&self, dialog: &ConvertDialog, cx: &mut Context<Self>) -> AnyElement {
        let folder = matches!(dialog.source, ConvertSource::Folder(_));
        let (title, note, note_color) = match (&dialog.source, &dialog.scan) {
            (ConvertSource::Current, _) => match &self.snapshot.path {
                Some(path) => (file_name(path), "当前播放的文件".to_string(), text_faint()),
                None => (
                    "没有正在播放的文件".to_string(),
                    "切换到「文件夹」可以批量转换整个目录".to_string(),
                    text_faint(),
                ),
            },
            (ConvertSource::Folder(dir), scan) => {
                let name = file_name(dir);
                match scan {
                    ScanState::Scanning => (name, "正在扫描子文件夹…".to_string(), text_faint()),
                    ScanState::Ready(files) if files.is_empty() => {
                        (name, "这里没有可转换的媒体文件".to_string(), ui::danger())
                    }
                    ScanState::Ready(files) => (
                        name,
                        format!("共 {} 个可转换文件（含子文件夹）", files.len()),
                        ui::accent(),
                    ),
                    ScanState::Failed(message) => (name, message.clone(), ui::danger()),
                    ScanState::Idle => (name, "等待选择文件夹".to_string(), text_faint()),
                }
            }
        };

        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(section_title("来源"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(convert_chip(
                        "convert-source-current",
                        "当前文件",
                        !folder,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.convert_use_current_file(cx);
                        }),
                    ))
                    .child(convert_chip(
                        "convert-source-folder",
                        "文件夹…",
                        folder,
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.convert_use_folder(window, cx);
                        }),
                    ))
                    // 换目录比重新选一遍更常见：这一行只在文件夹模式下出现。
                    .when(folder, |this| {
                        this.child(
                            div()
                                .id("convert-repick")
                                .px(px(6.0))
                                .py(px(3.0))
                                .rounded(px(6.0))
                                .text_size(px(11.0))
                                .text_color(text_muted())
                                .cursor(CursorStyle::PointingHand)
                                .hover(|style| style.bg(ui::surface_hover()))
                                .child("重新选择…")
                                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                    cx.stop_propagation();
                                    this.convert_pick_folder(window, cx);
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(text())
                            .child(ui::ellipsis(title)),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(note_color)
                            .child(ui::ellipsis(note)),
                    ),
            )
            .into_any_element()
    }

    /// 目标格式：视频、音频各一组格子，选中的那种描一道强调色的边。
    fn render_convert_formats(&self, dialog: &ConvertDialog, cx: &mut Context<Self>) -> AnyElement {
        let mut column = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(section_title("目标格式"))
            // 选中的格式和它的编码组合放最上面：格子铺了几屏，不能只靠高亮认。
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(ui::accent())
                    .whitespace_normal()
                    .child(format!(
                        "输出 .{} · {}",
                        dialog.target.extension(),
                        dialog.target.hint()
                    )),
            );

        for kind in [TargetKind::Video, TargetKind::Audio] {
            let formats: Vec<TargetFormat> = TargetFormat::ALL
                .iter()
                .copied()
                .filter(|format| format.kind() == kind)
                .collect();
            column = column.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pt(px(4.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text_faint())
                            .child(kind.label()),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(text_faint())
                            .child(format!("{} 种", formats.len())),
                    ),
            );
            for row in formats.chunks(COLUMNS) {
                column = column.child(
                    div()
                        .flex()
                        .gap(px(4.0))
                        .children(
                            row.iter()
                                .map(|format| render_format_chip(dialog.target, *format, cx)),
                        )
                        // 末行不满时补空位，格子宽度才和上面几行对得上。
                        .children((row.len()..COLUMNS).map(|_| div().flex_1().into_any_element())),
                );
            }
        }

        column
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(text_faint())
                    .whitespace_normal()
                    .child("只有 MKV 保留原字幕；其它容器不带字幕。全部重编码，耗时与片长相关。"),
            )
            .into_any_element()
    }

    fn render_convert_running(&self, dialog: &ConvertDialog, cx: &mut Context<Self>) -> AnyElement {
        let ConvertState::Running {
            index,
            total,
            name,
            ratio,
        } = &dialog.state
        else {
            return div().into_any_element();
        };
        let total = (*total).max(1);
        // 整批的进度 = 已完成项数 + 当前项的比例。总时长读不到时按已完成项数算，
        // 至少不会一直显示 0%。
        let overall = ((*index as f32) + ratio.unwrap_or(0.0)) / total as f32;
        let percent = (overall * 100.0).round().clamp(0.0, 100.0) as i32;

        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(text_muted())
                    .child(format!("正在转换 {} / {}", *index + 1, total)),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(text())
                    .child(ui::ellipsis(name.clone())),
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
                            .w(gpui_kit::relative(overall.clamp(0.0, 1.0))),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(ui::accent())
                    .child(format!("{percent}%")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .child(settings_button(
                        "convert-abort",
                        "取消",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.cancel_convert(cx);
                        }),
                    )),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(text_faint())
                    .whitespace_normal()
                    .child("转换在后台进行，关掉面板会取消并删除当前这一项的半成品。"),
            )
            .into_any_element()
    }

    fn render_convert_done(&self, dialog: &ConvertDialog, cx: &mut Context<Self>) -> AnyElement {
        let ConvertState::Done { summary, cancelled } = &dialog.state else {
            return div().into_any_element();
        };
        let successes = summary.outputs.len();
        let failures = summary.failures.len();
        let title = if *cancelled {
            "已取消"
        } else if failures == 0 {
            "转换完成"
        } else if successes == 0 {
            "转换失败"
        } else {
            "部分文件转换失败"
        };
        let title_color = if failures > 0 && !*cancelled {
            ui::danger()
        } else {
            text()
        };

        let mut stats = format!("成功 {successes} 个");
        if failures > 0 {
            stats.push_str(&format!(" · 失败 {failures} 个"));
        }
        if summary.skipped > 0 {
            stats.push_str(&format!(" · 未处理 {} 个", summary.skipped));
        }

        let has_output = successes > 0;
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(title_color)
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_muted())
                    .child(stats),
            );

        if let Some(dir) = summary.outputs.first().and_then(|path| path.parent()) {
            body = body.child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_faint())
                    .child(ui::ellipsis(dir.display().to_string())),
            );
        }

        if failures > 0 {
            let mut list = div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .p(px(10.0))
                .rounded(px(8.0))
                .bg(gpui_kit::rgba(0x2a1414f2));
            for failure in summary.failures.iter().take(MAX_FAILURES_SHOWN) {
                list = list.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(text())
                                .child(ui::ellipsis(file_name(&failure.input))),
                        )
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(ui::danger())
                                .whitespace_normal()
                                .child(brief_reason(&failure.message)),
                        ),
                );
            }
            if failures > MAX_FAILURES_SHOWN {
                list = list.child(div().text_size(px(10.0)).text_color(text_faint()).child(
                    format!("还有 {} 个失败未列出", failures - MAX_FAILURES_SHOWN),
                ));
            }
            body = body.child(list);
        }

        body.child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.0))
                .when(has_output, |this| {
                    this.child(settings_button(
                        "convert-reveal",
                        "定位",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.convert_reveal_result(cx);
                        }),
                    ))
                    .child(settings_button(
                        "convert-open",
                        "打开",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.convert_open_result(cx);
                        }),
                    ))
                })
                .child(primary_button(
                    "convert-finish",
                    "完成",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.close_convert_dialog(cx);
                    }),
                )),
        )
        .into_any_element()
    }
}

fn section_title(text: &'static str) -> AnyElement {
    div()
        .text_size(px(12.0))
        .text_color(text_muted())
        .child(text)
        .into_any_element()
}

/// 来源切换的两颗小按钮。选中那科用强调色填充，一眼看得出这一批从哪来。
fn convert_chip(
    id: &'static str,
    label: &'static str,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .h(px(28.0))
        .px(px(10.0))
        .rounded(px(7.0))
        .text_size(px(12.0))
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

/// 一个格式格子：只写扩展名，编码说明统一放在组标题下面（几十个格子逐张带说明
/// 会把面板撑成一条长长的清单）。
fn render_format_chip(
    selected: TargetFormat,
    format: TargetFormat,
    cx: &mut Context<PlayerApp>,
) -> AnyElement {
    let active = selected == format;
    div()
        .id(SharedString::from(format!(
            "convert-format-{}",
            format.extension()
        )))
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .items_center()
        .justify_center()
        .h(px(26.0))
        .rounded(px(6.0))
        .text_size(px(11.0))
        .bg(if active {
            ui::surface_active()
        } else {
            ui::surface_hover()
        })
        .border_1()
        .border_color(if active { ui::accent() } else { ui::track() })
        .text_color(if active { ui::accent() } else { text_muted() })
        .cursor(CursorStyle::PointingHand)
        .when(!active, |this| {
            this.hover(|style| style.bg(ui::surface_active()).text_color(text()))
        })
        .child(format.extension())
        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            cx.stop_propagation();
            this.convert_set_target(format, cx);
        }))
        .into_any_element()
}

/// 失败原因只取第一行、截到 80 字：ffmpeg 后面跟着一大段参数回显，面板放不下。
fn brief_reason(message: &str) -> String {
    let line = message
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(message)
        .trim();
    if line.chars().count() <= 80 {
        line.to_string()
    } else {
        let mut text: String = line.chars().take(79).collect();
        text.push('…');
        text
    }
}
