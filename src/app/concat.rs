//! 「拼接为单个文件」对话框：把当前播放列表里的文件按顺序接成一个。
//!
//! 直接拿列表当输入，不做文件挑选：用户已经把要拼的东西排好顺序放进来了，
//! 再让他挑一遍反而多一步。想调整顺序就先在列表里拖。

use super::*;
use crate::clip::{self as clip_core, ConcatEvent, ConcatJob, ConcatOutcome, ConcatRequest};
use std::path::Path;

/// 拼接任务的全部状态。
pub(super) struct ConcatDialog {
    /// 任务代次：后台任务回来时用它确认「这个事件还属于当前对话框」。
    generation: u64,
    inputs: Vec<PathBuf>,
    state: ConcatState,
    job: Option<Arc<ConcatJob>>,
}

enum ConcatState {
    Idle,
    Running { progress: f32 },
    Done(PathBuf),
    Failed(String),
}

impl PlayerApp {
    pub(super) fn open_concat_dialog(&mut self, cx: &mut Context<Self>) {
        if self.concat.is_some() {
            self.close_concat_dialog(cx);
            return;
        }
        let playlist = self.playlist();
        let inputs: Vec<PathBuf> = (0..playlist.len())
            .filter_map(|index| playlist.playable_path(index).cloned())
            .collect();
        if inputs.len() < 2 {
            self.flash("列表里至少要有两个文件，才好拼接");
            cx.notify();
            return;
        }

        // 这几个对话框同一时间只留一个：都是「对当前文件做件大事」。
        self.close_remux_dialog(cx);
        self.close_metadata_dialog(cx);
        self.close_proxy_dialog(cx);
        self.close_scope_dialog(cx);
        self.close_scenes_dialog(cx);
        self.close_multicam_dialog(cx);
        self.close_clip_dialog(cx);

        self.concat_generation += 1;
        self.concat = Some(ConcatDialog {
            generation: self.concat_generation,
            inputs,
            state: ConcatState::Idle,
            job: None,
        });
        self.menu = None;
        self.menu_bar = None;
        cx.notify();
    }

    pub(super) fn close_concat_dialog(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.concat.take()
            && let Some(job) = &dialog.job
        {
            job.cancel();
        }
        cx.notify();
    }

    /// 起一个后台任务。命不中 ffmpeg 时留在表单态，把原因显示出来。
    fn start_concat(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.concat.as_mut() else {
            return;
        };
        if !matches!(dialog.state, ConcatState::Idle | ConcatState::Failed(_)) {
            return;
        }
        let Some(first) = dialog.inputs.first() else {
            return;
        };
        let request = ConcatRequest {
            inputs: dialog.inputs.clone(),
            output: clip_core::concat_output_path(first),
        };
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        match ConcatJob::start(request, tx) {
            Ok(job) => {
                dialog.job = Some(Arc::new(job));
                dialog.state = ConcatState::Running { progress: 0.0 };
            }
            Err(error) => {
                dialog.state = ConcatState::Failed(format!("{error:#}"));
                cx.notify();
                return;
            }
        }
        let generation = dialog.generation;
        cx.notify();

        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let _ = this.update(cx, |this, cx| {
                    let Some(dialog) = this.concat.as_mut() else {
                        return;
                    };
                    // 旧任务可能在新对话框打开之后才收尾：只认当前代次。
                    if dialog.generation != generation {
                        return;
                    }
                    match event {
                        ConcatEvent::Progress(value) => {
                            if let ConcatState::Running { progress } = &mut dialog.state {
                                *progress = value;
                            }
                        }
                        ConcatEvent::Done(outcome) => {
                            dialog.job = None;
                            dialog.state = match outcome {
                                ConcatOutcome::Finished(path) => ConcatState::Done(path),
                                ConcatOutcome::Cancelled => ConcatState::Idle,
                                ConcatOutcome::Failed(message) => ConcatState::Failed(message),
                            };
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(super) fn render_concat_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.concat.as_ref()?;
        let body = match &dialog.state {
            ConcatState::Idle => self.render_concat_form(dialog, None, cx),
            ConcatState::Failed(message) => self.render_concat_form(dialog, Some(message), cx),
            ConcatState::Running { progress } => render_concat_progress(*progress, cx),
            ConcatState::Done(path) => self.render_concat_done(path, cx),
        };
        Some(modal_card(
            "concat-dialog",
            "拼接为单个文件",
            body,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.close_concat_dialog(cx);
            }),
        ))
    }

    fn render_concat_form(
        &self,
        dialog: &ConcatDialog,
        error: Option<&String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = dialog.inputs.len();
        let mut body = div().flex().flex_col().gap(px(10.0)).child(
            div()
                .text_size(px(12.0))
                .text_color(text())
                .child(format!("按列表顺序拼接 {count} 个文件")),
        );

        // 只列前几个：列表再长也不该把对话框撑成半个屏幕。
        for input in dialog.inputs.iter().take(5) {
            body = body.child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_muted())
                    .child(file_name(input)),
            );
        }
        if count > 5 {
            body = body.child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_faint())
                    .child(format!("…等另外 {} 个", count - 5)),
            );
        }

        body = body.child(
            div()
                .text_size(px(10.0))
                .text_color(text_faint())
                .whitespace_normal()
                .child(
                    "不重新编码，几秒就能完成；前提是这些文件的分辨率、帧率、编码一致。\
                     参数不同的文件先用「格式转换」统一一遍，否则接缝会花屏或直接失败。",
                ),
        );

        if let Some(message) = error {
            body = body.child(
                div()
                    .p(px(10.0))
                    .rounded(px(8.0))
                    .bg(gpui_kit::rgba(0x2a1414f2))
                    .text_size(px(11.0))
                    .text_color(gpui_kit::rgb(0xff9a9a))
                    .whitespace_normal()
                    .child(message.clone()),
            );
        }

        body.child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.0))
                .child(settings_button(
                    "concat-cancel",
                    "取消",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.close_concat_dialog(cx);
                    }),
                ))
                .child(primary_button(
                    "concat-start",
                    "开始拼接",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.start_concat(cx);
                    }),
                )),
        )
        .into_any_element()
    }

    fn render_concat_done(&self, path: &Path, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(text())
                    .child("拼接完成"),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_muted())
                    .whitespace_normal()
                    .child(path.display().to_string()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.0))
                    .child(settings_button(
                        "concat-reveal",
                        "定位",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.concat_reveal_result(cx);
                        }),
                    ))
                    .child(settings_button(
                        "concat-open",
                        "打开",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.concat_open_result(cx);
                        }),
                    ))
                    .child(primary_button(
                        "concat-finish",
                        "完成",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.close_concat_dialog(cx);
                        }),
                    )),
            )
            .into_any_element()
    }

    fn concat_open_result(&mut self, cx: &mut Context<Self>) {
        let Some(ConcatState::Done(path)) = self.concat.as_ref().map(|dialog| &dialog.state) else {
            return;
        };
        match platform::open::open(path) {
            Ok(()) => self.flash(format!("已打开 {}", path.display())),
            Err(message) => self.flash(message),
        }
        cx.notify();
    }

    fn concat_reveal_result(&mut self, cx: &mut Context<Self>) {
        let Some(ConcatState::Done(path)) = self.concat.as_ref().map(|dialog| &dialog.state) else {
            return;
        };
        match platform::reveal::reveal(path) {
            Ok(()) => self.flash("已在文件管理器里定位拼接文件"),
            Err(message) => self.flash(message),
        }
        cx.notify();
    }
}

fn render_concat_progress(progress: f32, cx: &mut Context<PlayerApp>) -> AnyElement {
    let percent = (progress * 100.0).round().clamp(0.0, 100.0) as i32;
    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(
            div()
                .text_size(px(12.0))
                .text_color(text_muted())
                .child(format!("正在拼接… {percent}%")),
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
                    "concat-abort",
                    "取消",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.close_concat_dialog(cx);
                    }),
                )),
        )
        .child(
            div()
                .text_size(px(10.0))
                .text_color(text_faint())
                .child("关掉对话框会取消并删除半成品。"),
        )
        .into_any_element()
}
