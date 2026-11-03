//! 镜头分割对话框：整片扫一遍，把切点列出来，点一下就跳过去。
//!
//! 切点边扫边出，长片不用等到最后才看得到结果；切换灵敏度会重跑检测，旧任务先取消。
//! 检测要解码整片，所以进度条、取消与「已找到几处」必须一直在屏幕上。

use super::*;
use crate::scenes::{self as scenes_core, SceneEvent, SceneJob, SceneOutcome};

#[derive(Clone, Debug)]
enum ScenesState {
    Running {
        progress: f32,
    },
    /// `cancelled` 为真表示中途取消，列表里只有扫到的那部分。
    Done {
        cancelled: bool,
    },
    Failed(String),
}

pub(super) struct ScenesDialog {
    generation: u64,
    path: PathBuf,
    duration: f64,
    threshold: f32,
    /// 已发现的切点，按时间先后。
    cuts: Vec<f64>,
    state: ScenesState,
    job: Option<Arc<SceneJob>>,
}

/// 灵敏度档位：分数越高越严格，0.6 基本只认画面整体换掉的那种切换。
const THRESHOLDS: [(f32, &str, &str); 3] = [
    (0.2, "灵敏", "scenes-threshold-low"),
    (
        scenes_core::DEFAULT_THRESHOLD,
        "默认",
        "scenes-threshold-mid",
    ),
    (0.6, "严格", "scenes-threshold-high"),
];

impl PlayerApp {
    pub(super) fn open_scenes_dialog(&mut self, cx: &mut Context<Self>) {
        if self.scenes.is_some() {
            self.close_scenes_dialog(cx);
            return;
        }
        let Some(path) = self.snapshot.path.clone() else {
            self.flash("没有正在播放的文件");
            cx.notify();
            return;
        };
        if !self.snapshot.has_video() {
            self.flash("纯音频文件没有画面");
            cx.notify();
            return;
        }
        if self.snapshot.duration <= 0.0 {
            self.flash("还没读出这个文件的时长，稍等一下再试");
            cx.notify();
            return;
        }

        self.close_remux_dialog(cx);
        self.close_metadata_dialog(cx);
        self.close_proxy_dialog(cx);
        self.close_scope_dialog(cx);
        self.close_multicam_dialog(cx);
        self.close_clip_dialog(cx);
        self.close_concat_dialog(cx);
        self.scenes_generation += 1;
        self.scenes = Some(ScenesDialog {
            generation: self.scenes_generation,
            path,
            duration: self.snapshot.duration,
            threshold: scenes_core::DEFAULT_THRESHOLD,
            cuts: Vec::new(),
            state: ScenesState::Running { progress: 0.0 },
            job: None,
        });
        self.menu = None;
        self.menu_bar = None;
        self.start_scenes(cx);
    }

    /// 关闭对话框。正在扫的任务一并取消：留它在后台解码整片没有意义。
    pub(super) fn close_scenes_dialog(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.scenes.take()
            && let Some(job) = &dialog.job
        {
            job.cancel();
        }
        cx.notify();
    }

    pub(super) fn sync_scenes_source(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.scenes.as_ref() else {
            return;
        };
        if self.snapshot.path.as_deref() != Some(dialog.path.as_path()) {
            self.close_scenes_dialog(cx);
        }
    }

    pub(super) fn on_scenes_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if self.scenes.is_none() {
            return false;
        }
        if event.keystroke.key == "escape" {
            self.close_scenes_dialog(cx);
            return true;
        }
        false
    }

    /// 换一档灵敏度就重扫：旧任务先停掉，旧结果清掉，否则两批切点会混在一起。
    fn set_scene_threshold(&mut self, threshold: f32, cx: &mut Context<Self>) {
        let Some(dialog) = self.scenes.as_mut() else {
            return;
        };
        dialog.threshold = threshold;
        self.start_scenes(cx);
    }

    /// 启动（或重启）一次检测。
    fn start_scenes(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.scenes.as_mut() else {
            return;
        };
        if let Some(job) = dialog.job.take() {
            job.cancel();
        }
        dialog.cuts.clear();
        dialog.state = ScenesState::Running { progress: 0.0 };
        let request = scenes_core::SceneRequest {
            input: dialog.path.clone(),
            duration: dialog.duration,
            threshold: dialog.threshold,
        };
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        match scenes_core::SceneJob::start(request, tx) {
            Ok(job) => dialog.job = Some(Arc::new(job)),
            Err(error) => {
                dialog.state = ScenesState::Failed(format!("{error:#}"));
                cx.notify();
                return;
            }
        }
        let generation = dialog.generation;
        cx.notify();

        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let _ = this.update(cx, |this, cx| {
                    let Some(dialog) = this.scenes.as_mut() else {
                        return;
                    };
                    if dialog.generation != generation {
                        return;
                    }
                    match event {
                        SceneEvent::Progress(progress) => {
                            if let ScenesState::Running { progress: current } = &mut dialog.state {
                                *current = progress;
                            }
                        }
                        SceneEvent::Found(time) => {
                            let time = time.max(0.0);
                            // 同一处可能被回报两次（重复帧、多路流），别堆重复项。
                            if !dialog.cuts.iter().any(|cut| (*cut - time).abs() < 0.01) {
                                dialog.cuts.push(time);
                            }
                        }
                        SceneEvent::Done(outcome) => {
                            dialog.job = None;
                            dialog.state = match outcome {
                                SceneOutcome::Finished => ScenesState::Done { cancelled: false },
                                SceneOutcome::Cancelled => ScenesState::Done { cancelled: true },
                                SceneOutcome::Failed(message) => ScenesState::Failed(message),
                            };
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// 取消扫描，保留已经找到的切点。
    fn cancel_scenes(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.scenes.as_mut()
            && let Some(job) = &dialog.job
        {
            job.cancel();
        }
        cx.notify();
    }

    /// 跳到某个切点并暂停：核对镜头边界要看的是这一帧，不是接着往后放。
    fn jump_to_scene(&mut self, time: f64, cx: &mut Context<Self>) {
        self.send(Command::SeekAbsolute(time.max(0.0)));
        self.send(Command::SetPaused(true));
        self.poke();
        cx.notify();
    }

    pub(super) fn render_scenes_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.scenes.as_ref()?;
        let mut body = div().flex().flex_col().gap(px(10.0)).child(
            div()
                .text_size(px(11.0))
                .text_color(text_muted())
                .child(format!(
                    "{} · 全长 {}",
                    file_name(&dialog.path),
                    format_time(dialog.duration)
                )),
        );

        let mut chips = div().flex().items_center().gap(px(6.0));
        for (threshold, label, id) in THRESHOLDS {
            chips = chips.child(chip(
                id,
                label,
                dialog.threshold == threshold,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.set_scene_threshold(threshold, cx);
                }),
            ));
        }
        body = body.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(text_faint())
                        .child("灵敏度"),
                )
                .child(chips),
        );

        if let ScenesState::Running { progress } = &dialog.state {
            let percent = (progress * 100.0).round().clamp(0.0, 100.0) as i32;
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text_muted())
                            .child(format!("正在扫描整片… {percent}%")),
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
                    ),
            );
        }

        if let ScenesState::Failed(message) = &dialog.state {
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

        body = body.child(cut_list(dialog, cx));

        let status = match &dialog.state {
            ScenesState::Running { .. } => format!("已找到 {} 处切换", dialog.cuts.len()),
            ScenesState::Done { cancelled: true } => {
                format!("已取消，留下 {} 处切换", dialog.cuts.len())
            }
            ScenesState::Done { .. } => format!("共 {} 处切换", dialog.cuts.len()),
            ScenesState::Failed(_) => String::new(),
        };
        let mut buttons = div().flex().items_center().gap(px(8.0));
        if matches!(dialog.state, ScenesState::Running { .. }) {
            buttons = buttons.child(settings_button(
                "scenes-cancel",
                "取消",
                true,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.cancel_scenes(cx);
                }),
            ));
        }
        buttons = buttons.child(primary_button(
            "scenes-close",
            "关闭",
            true,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.close_scenes_dialog(cx);
            }),
        ));

        Some(modal_card(
            "scenes-dialog",
            "镜头分割",
            body.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text_faint())
                            .child(status),
                    )
                    .child(buttons),
            )
            .into_any_element(),
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.close_scenes_dialog(cx);
            }),
        ))
    }
}

/// 切点列表，点一行跳到那一帧。
fn cut_list(dialog: &ScenesDialog, cx: &mut Context<PlayerApp>) -> AnyElement {
    if dialog.cuts.is_empty() {
        let text = match &dialog.state {
            ScenesState::Running { .. } => "还没找到切换点，正在扫。",
            ScenesState::Done { cancelled: true } => "已取消，没扫到的部分没有结果。",
            ScenesState::Done { .. } => "没检测到明显切换；换成「灵敏」再试一次。",
            ScenesState::Failed(_) => "",
        };
        return div()
            .text_size(px(11.0))
            .text_color(text_faint())
            .whitespace_normal()
            .child(text)
            .into_any_element();
    }

    let mut list = div()
        .id("scenes-cut-list")
        .flex()
        .flex_col()
        .gap(px(2.0))
        .max_h(px(240.0))
        .overflow_y_scroll();
    for (index, time) in dialog.cuts.iter().enumerate() {
        let time = *time;
        list = list.child(
            div()
                .id(SharedString::from(format!("scene-cut-{index}")))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(6.0))
                .cursor(CursorStyle::PointingHand)
                .hover(|style| style.bg(gpui_kit::rgba(0x1c1c22ff)))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(text())
                        .child(format_time(time)),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(text_faint())
                        .child(format!("第 {} 个镜头", index + 1)),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.jump_to_scene(time, cx);
                })),
        );
    }
    list.into_any_element()
}
