//! 裁切面板的批处理：来源切换、文件夹扫描、启动与取消，以及这几块界面。
//!
//! 批处理复用面板上那一整套导出参数（包括预设），只是把「按区间裁一个文件」换成
//! 「把整个文件夹逐个整段导出」。区间类的操作（删除中间段、分割、预览、烧字幕）
//! 在批处理里没有意义，界面里整块收掉，任务侧也强制按「保留整段」走。

use super::widgets::{note, note_owned, option_chip, section_title};
use super::*;
use crate::clip as clip_core;

impl PlayerApp {
    /// 切回「当前文件」：回到按区间裁切的模式。
    pub(super) fn clip_use_current(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.clip.as_mut() else {
            return;
        };
        if !dialog.batch() {
            return;
        }
        dialog.source = ClipSource::Current;
        dialog.state = ClipState::Editing;
        cx.notify();
    }

    /// 切到「文件夹」：已经选过就保留，没选过就弹选择框。
    pub(super) fn clip_use_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let already = self
            .clip
            .as_ref()
            .is_some_and(|dialog| matches!(dialog.source, ClipSource::Folder(_)));
        if already {
            return;
        }
        self.clip_pick_folder(window, cx);
    }

    /// 选一个文件夹（含子文件夹）作为批处理来源。选中后立刻在后台扫描。
    pub(super) fn clip_pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let mut picker = rfd::FileDialog::new().set_parent(window);
        if let Some(dir) = &self.settings.last_dir {
            picker = picker.set_directory(dir);
        }
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(picker.pick_folder());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if let Some(folder) = picked {
                    this.remember_dir(&folder);
                    this.clip_start_scan(folder, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 后台扫描文件夹。代次一变，之前那次扫描的结果就作废。
    fn clip_start_scan(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        let Some(dialog) = self.clip.as_mut() else {
            return;
        };
        dialog.scan_generation += 1;
        let generation = dialog.scan_generation;
        dialog.source = ClipSource::Folder(folder.clone());
        dialog.scan = ClipScan::Scanning;
        // 批处理只做整段导出，区间类的开关先归位，免得切回单文件时还带着旧选择。
        dialog.kind = clip_core::CutKind::Keep;
        dialog.preview = None;
        dialog.state = ClipState::Editing;
        cx.notify();

        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            // 几百个文件的目录要读一会儿，不能放在界面线程上。
            let _ = tx.send(crate::convert::scan_folder(&folder));
        });

        cx.spawn(async move |this, cx| {
            // 通道断了（线程没起来）也当成读不了，总比一直停在「正在扫描」强。
            let scanned = rx
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("扫描线程提前退出")));
            let _ = this.update(cx, |this, cx| {
                let Some(dialog) = this.clip.as_mut() else {
                    return;
                };
                // 用户可能已经换了文件夹（甚至关了面板）：只认当前代次。
                if dialog.scan_generation != generation {
                    return;
                }
                dialog.scan = match scanned {
                    Ok(files) => ClipScan::Ready(files),
                    Err(error) => ClipScan::Failed(format!("{error:#}")),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// 启动批处理。校验失败时留在编辑态，把原因写进面板。
    pub(super) fn start_clip_batch(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.clip.as_ref() else {
            return;
        };
        if !dialog.editing() || !dialog.batch() {
            return;
        }
        let inputs = dialog.batch_inputs().to_vec();
        if inputs.is_empty() {
            let message = match &dialog.scan {
                ClipScan::Scanning => "文件夹还在扫描，稍等一下",
                ClipScan::Failed(_) => "这个文件夹读不了，换一个试试",
                _ => "这个文件夹里没有可处理的媒体文件",
            };
            self.flash(message);
            cx.notify();
            return;
        }
        let format = dialog.options.format;
        let items = clip_core::plan_batch(&inputs, format);

        let Some(dialog) = self.clip.as_mut() else {
            return;
        };
        let options = match resolve_clip_options(dialog, cx) {
            Ok(options) => options,
            Err(message) => {
                dialog.state = ClipState::Failed(message);
                cx.notify();
                return;
            }
        };

        let total = items.len();
        let first = items[0].input.clone();
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        match clip_core::BatchJob::start(items, options, tx) {
            Ok(job) => {
                dialog.batch = Some(Arc::new(job));
                dialog.state = ClipState::BatchRunning {
                    index: 0,
                    total,
                    name: file_name(&first),
                    // 第一个文件的时长还没探出来，进度条先不猜。
                    ratio: None,
                    done: 0,
                    failed: 0,
                };
                dialog.preview = None;
            }
            // 进程都没起来，不会有任何事件，别去等一个空通道。
            Err(error) => {
                dialog.state = ClipState::Failed(format!("{error:#}"));
                cx.notify();
                return;
            }
        }
        let generation = dialog.generation;
        cx.notify();

        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let _ = this.update(cx, |this, cx| {
                    let Some(dialog) = this.clip.as_mut() else {
                        return;
                    };
                    // 换了对话框（或点了取消）之后，旧批次的回包一律丢掉。
                    if dialog.generation != generation {
                        return;
                    }
                    match event {
                        clip_core::BatchEvent::ItemStarted { index, input } => {
                            if let ClipState::BatchRunning {
                                index: current,
                                name,
                                ratio,
                                ..
                            } = &mut dialog.state
                            {
                                *current = index;
                                *name = file_name(&input);
                                *ratio = None;
                            }
                        }
                        clip_core::BatchEvent::Progress { index, ratio } => {
                            if let ClipState::BatchRunning {
                                index: current,
                                ratio: current_ratio,
                                ..
                            } = &mut dialog.state
                                && *current == index
                            {
                                *current_ratio = Some(ratio);
                            }
                        }
                        clip_core::BatchEvent::ItemFinished { ok, .. } => {
                            if let ClipState::BatchRunning { done, failed, .. } = &mut dialog.state
                            {
                                if ok {
                                    *done += 1;
                                } else {
                                    *failed += 1;
                                }
                            }
                        }
                        clip_core::BatchEvent::Finished(outcome) => {
                            dialog.batch = None;
                            dialog.state = match outcome {
                                clip_core::BatchOutcome::Done(summary) => ClipState::BatchDone {
                                    summary,
                                    cancelled: false,
                                },
                                clip_core::BatchOutcome::Cancelled(summary) => {
                                    ClipState::BatchDone {
                                        summary,
                                        cancelled: true,
                                    }
                                }
                            };
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// 取消批处理，回到编辑态继续调参数。
    ///
    /// 面板不关：一批文件可能已经跑了几分种，用户多半只是想改个参数重来。
    /// 回调线程随后还会送事件过来，靠代次让它们失效。
    pub(super) fn cancel_clip_batch(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.clip.as_mut() else {
            return;
        };
        if let Some(job) = dialog.batch.take() {
            job.cancel();
        }
        dialog.generation += 1;
        dialog.state = ClipState::Editing;
        cx.notify();
    }

    /// 从「完成」回到编辑态：换套参数把这一批重跑一遍，文件夹不用重新选。
    pub(super) fn clip_batch_edit_again(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.clip.as_mut() {
            dialog.state = ClipState::Editing;
            cx.notify();
        }
    }

    /// 「来源」一块：当前文件 / 文件夹，以及扫描结果。
    pub(super) fn render_clip_source(
        &self,
        dialog: &ClipDialog,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let folder = match &dialog.source {
            ClipSource::Folder(path) => Some(path),
            ClipSource::Current => None,
        };

        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(section_title("来源"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(option_chip(
                        "clip-source-current",
                        "当前文件",
                        folder.is_none(),
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.clip_use_current(cx);
                        }),
                    ))
                    .child(option_chip(
                        "clip-source-folder",
                        "文件夹…",
                        folder.is_some(),
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.clip_use_folder(window, cx);
                        }),
                    )),
            );

        if let Some(folder) = folder {
            body = body.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text())
                            .child(ui::ellipsis(folder.display().to_string())),
                    )
                    .child(
                        div()
                            .id("clip-batch-reselect")
                            .text_size(px(11.0))
                            .text_color(text_faint())
                            .cursor(CursorStyle::PointingHand)
                            .hover(|style| style.text_color(ui::accent()))
                            .child("重新选择…")
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                this.clip_pick_folder(window, cx);
                            })),
                    ),
            );
            body = body.child(match &dialog.scan {
                ClipScan::Scanning => note("正在扫描文件夹…"),
                ClipScan::Ready(files) if files.is_empty() => {
                    note("这个文件夹里没有可处理的媒体文件；子文件夹也会扫，换一个目录试试")
                }
                ClipScan::Ready(files) => note_owned(format!(
                    "共 {} 个文件（含子文件夹），逐个整段导出",
                    files.len()
                )),
                ClipScan::Failed(message) => div()
                    .text_size(px(11.0))
                    .text_color(ui::danger())
                    .whitespace_normal()
                    .child(message.clone())
                    .into_any_element(),
            });
        }

        body.into_any_element()
    }

    /// 批处理运行中：第几项、当前项进度、已完成与失败的账目。
    pub(super) fn render_clip_batch_progress(
        &self,
        dialog: &ClipDialog,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ClipState::BatchRunning {
            index,
            total,
            name,
            ratio,
            done,
            failed,
        } = &dialog.state
        else {
            return div().into_any_element();
        };
        let (index, total, done, failed) = (*index, *total, *done, *failed);
        let current = (index + 1).min(total);
        // 整批进度：已收尾的项 + 当前项的比例。当前项还没报进度时按 0 算，
        // 比一路跳来跳去诚实。
        let overall = ((index as f32 + ratio.unwrap_or(0.0)) / total.max(1) as f32).clamp(0.0, 1.0);
        let percent = (overall * 100.0).round() as i32;

        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(text_muted())
                    .child(format!("正在批处理 {current} / {total}：{name}")),
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
                            .w(gpui_kit::relative(overall)),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_muted())
                    .child(format!("{percent}%　已完成 {done} · 失败 {failed}")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .child(settings_button(
                        "clip-batch-abort",
                        "取消",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.cancel_clip_batch(cx);
                        }),
                    )),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(text_faint())
                    .whitespace_normal()
                    .child("逐个文件串行处理；取消只停掉当前这一项，已完成的文件留在原处。"),
            )
            .into_any_element()
    }

    /// 批处理收尾：成功、失败、没来得及处理的项。
    pub(super) fn render_clip_batch_done(
        &self,
        summary: &clip_core::BatchSummary,
        cancelled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut body = div().flex().flex_col().gap(px(10.0)).child(
            div()
                .text_size(px(13.0))
                .text_color(text())
                .child(if cancelled {
                    format!("已取消：完成 {} 个文件", summary.outputs.len())
                } else if summary.failures.is_empty() {
                    format!("批处理完成：{} 个文件", summary.outputs.len())
                } else {
                    format!(
                        "批处理结束：成功 {} 个，失败 {} 个",
                        summary.outputs.len(),
                        summary.failures.len()
                    )
                }),
        );

        if summary.skipped > 0 {
            body = body.child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_muted())
                    .child(format!("还有 {} 个文件没有处理", summary.skipped)),
            );
        }

        // 失败原因一条一行地列：批处理最常见的问题是某一个文件读不出来，
        // 不写清楚是哪一条，用户只能自己一个个试。
        for failure in summary.failures.iter().take(5) {
            body = body.child(
                div()
                    .text_size(px(11.0))
                    .text_color(ui::danger())
                    .whitespace_normal()
                    .child(format!(
                        "{}：{}",
                        failure.input.display(),
                        crate::app::widgets::brief_reason(&failure.message)
                    )),
            );
        }
        if summary.failures.len() > 5 {
            body = body.child(
                div()
                    .text_size(px(10.0))
                    .text_color(text_faint())
                    .child(format!(
                        "还有 {} 条失败记录没有列出",
                        summary.failures.len() - 5
                    )),
            );
        }

        if let Some(first) = summary.outputs.first() {
            let dir = first
                .parent()
                .map(|dir| dir.display().to_string())
                .unwrap_or_default();
            body = body.child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_muted())
                    .whitespace_normal()
                    .child(format!("产物在 {dir}，名字是「原名 - 批处理」")),
            );
        }

        body.child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.0))
                // 换套参数重跑很常见（先导小体积看看效果），不用重新选文件夹。
                .child(settings_button(
                    "clip-batch-again",
                    "返回设置",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.clip_batch_edit_again(cx);
                    }),
                ))
                .when(!summary.outputs.is_empty(), |this| {
                    this.child(settings_button(
                        "clip-batch-reveal",
                        "定位",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.clip_reveal_result(cx);
                        }),
                    ))
                    .child(settings_button(
                        "clip-batch-open",
                        "打开",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.clip_open_result(cx);
                        }),
                    ))
                })
                .child(primary_button(
                    "clip-batch-finish",
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
