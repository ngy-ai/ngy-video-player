//! 转换面板的渲染：来源、目标格式、进度与结果。
//!
//! 与播放列表共用右侧通道，所以它挤掉列表、并按自己的宽度让画面与控制条让位。
//! 面板不是模态的：用户可以一边看一边挑格式，转过的那一集还能接着播。

use super::super::*;
use super::state::{ConvertDialog, ConvertSource, ConvertState, ScanState};
use crate::convert::TargetFormat;

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
                // 正文与格式下拉叠在同一块区域：下拉展开时整块盖住正文。
                // 这样列表不会被滚动容器裁掉，也不用去算触发器在屏幕上的位置。
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h(px(0.0))
                        .child(
                            div()
                                .id("convert-panel-body")
                                .absolute()
                                .inset_0()
                                .overflow_y_scroll()
                                .px(px(14.0))
                                .pb(px(14.0))
                                .child(body),
                        )
                        .when(dialog.picker.open, |this| {
                            this.child(self.render_convert_picker(dialog, cx))
                        }),
                )
                .children(footer)
                .into_any_element(),
        )
    }

    fn render_convert_editor(&self, dialog: &ConvertDialog, cx: &mut Context<Self>) -> AnyElement {
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(self.render_convert_source(dialog, cx));

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

        body.child(
            div()
                .text_size(px(10.0))
                .text_color(text_faint())
                .whitespace_normal()
                .child("只有 MKV 保留原字幕；其它容器不带字幕。全部重编码，耗时与片长相关。"),
        )
        .into_any_element()
    }

    /// 面板底部的固定操作区：格式下拉 + 说明 + 开始按钮。
    fn render_convert_footer(&self, dialog: &ConvertDialog, cx: &mut Context<Self>) -> AnyElement {
        let picking = dialog.picker.open;
        // 有文件可转才让按钮亮起来：点下去什么都没发生的按钮最让人困惑。
        let ready = dialog.editing() && self.convert_input_count() > 0 && !picking;

        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .px(px(14.0))
            .py(px(10.0))
            .border_t_1()
            .border_color(gpui_kit::rgb(0x26262e))
            .child(self.render_format_trigger(dialog, cx))
            // 列表展开时把说明和按钮让出去：这时候视线都在列表上，不该开始转换。
            .when(!picking, |this| {
                this.child(
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
            })
            .into_any_element()
    }

    /// 目标格式的入口：平时只显示当前值，点开才是带搜索的列表。
    fn render_format_trigger(&self, dialog: &ConvertDialog, cx: &mut Context<Self>) -> AnyElement {
        let open = dialog.picker.open;
        div()
            .id("convert-format-trigger")
            .flex()
            .items_center()
            .gap(px(8.0))
            .h(px(32.0))
            .px(px(10.0))
            .rounded(px(7.0))
            .bg(if open {
                ui::surface_active()
            } else {
                ui::surface_hover()
            })
            .border_1()
            .border_color(if open { ui::accent() } else { ui::track() })
            .cursor(CursorStyle::PointingHand)
            .when(!open, |this| {
                this.hover(|style| style.bg(ui::surface_active()))
            })
            .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                cx.stop_propagation();
                this.convert_toggle_picker(cx);
            }))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(text_faint())
                            .child("输出格式"),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(ui::accent())
                            .child(format!(".{}", dialog.target.extension())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(10.0))
                            .text_color(text_faint())
                            .child(ui::ellipsis(dialog.target.hint())),
                    ),
            )
            .child(icon(IconName::ChevronDown, 14.0, text_muted()))
            .into_any_element()
    }

    /// 格式下拉：搜索框 + 命中列表，盖在面板正文上。
    fn render_convert_picker(&self, dialog: &ConvertDialog, cx: &mut Context<Self>) -> AnyElement {
        let matches = dialog.matches();
        let query = &dialog.picker.query;

        let mut list = div().flex().flex_col().gap(px(2.0));
        if matches.is_empty() {
            list = list.child(
                div()
                    .py(px(10.0))
                    .text_size(px(11.0))
                    .text_color(text_faint())
                    .child("没有匹配的格式，换个词试试"),
            );
        }
        for target in &matches {
            list = list.child(render_picker_row(dialog.target, *target, cx));
        }

        div()
            .id("convert-picker")
            .absolute()
            .inset_0()
            // 底色必须不透明：底下还压着来源区，透出来就是两层文字叠在一起。
            .bg(gpui_kit::rgb(0x131317))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .px(px(14.0))
            .py(px(12.0))
            .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                cx.stop_propagation();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(30.0))
                    .px(px(8.0))
                    .rounded(px(7.0))
                    .bg(ui::surface_hover())
                    .border_1()
                    .border_color(ui::accent())
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(if query.is_empty() {
                                text_faint()
                            } else {
                                text()
                            })
                            .child(if query.is_empty() {
                                "搜索格式：mkv / h264 / 无损…".to_string()
                            } else {
                                query.clone()
                            }),
                    )
                    // 光标固定画在末尾：这里不做左右移动与选区，输入永远是追加/退格。
                    .child(div().w(px(1.0)).h(px(13.0)).bg(ui::accent())),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(text_faint())
                            .child(format!("{} 个格式", matches.len())),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(text_faint())
                            .child("回车选第一个 · Esc 关闭"),
                    ),
            )
            .child(
                div()
                    .id("convert-picker-list")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .child(list),
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

/// 下拉列表里的一行：扩展名 + 编码说明 + 视频/音频标签。
fn render_picker_row(
    selected: TargetFormat,
    format: TargetFormat,
    cx: &mut Context<PlayerApp>,
) -> AnyElement {
    let active = selected == format;
    // 选中行底色是强调色，浅色调的文字在上面会糊成一片，统一换成深色。
    let ink: Hsla = if active {
        gpui_kit::rgb(0x111114).into()
    } else {
        text_faint()
    };
    div()
        .id(SharedString::from(format!(
            "convert-format-{}",
            format.extension()
        )))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .h(px(30.0))
        .px(px(8.0))
        .rounded(px(6.0))
        .cursor(CursorStyle::PointingHand)
        .when(active, |this| {
            this.bg(ui::accent()).text_color(gpui_kit::rgb(0x111114))
        })
        .when(!active, |this| {
            this.text_color(text())
                .hover(|style| style.bg(ui::surface_hover()))
        })
        .child(
            div()
                .flex()
                .flex_1()
                .min_w(px(0.0))
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .w(px(44.0))
                        .text_size(px(12.0))
                        .child(format.extension()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(10.0))
                        .text_color(ink)
                        .child(ui::ellipsis(format.hint())),
                ),
        )
        .child(
            div()
                .text_size(px(9.0))
                .text_color(ink)
                .child(format.kind().label()),
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            cx.stop_propagation();
            this.convert_pick_format(format, cx);
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
