//! 双机位并排导出对话框：选第二个机位、对齐偏移、导出 1280×720 的拼屏。
//!
//! 对齐只做「右机位跳过开头」一个参数。双机位拍摄里 B 机常常先开机，剪掉
//! 开头几秒就能和 A 机对上；反过来把两台机器的文件顺序换一下即可。

use super::*;
use crate::multicam::{
    self as multicam_core, MulticamEvent, MulticamJob, MulticamOutcome, MulticamRequest,
};
use std::path::Path;

#[derive(Clone, Debug)]
enum MulticamState {
    Idle,
    Running { progress: f32 },
    Done(PathBuf),
}

pub(super) struct MulticamDialog {
    generation: u64,
    left: PathBuf,
    left_duration: f64,
    /// 第二个机位：路径 + 时长。
    right: Option<(PathBuf, f64)>,
    /// 右机位跳过开头的秒数。
    right_skip: f64,
    state: MulticamState,
    /// 选文件 / 导出失败的原因，显示在按钮上方。
    message: Option<String>,
    job: Option<Arc<MulticamJob>>,
}

impl MulticamDialog {
    fn overlap(&self) -> Option<f64> {
        let (_, right_duration) = self.right.as_ref()?;
        multicam_core::overlap(self.left_duration, *right_duration, self.right_skip)
    }
}

impl PlayerApp {
    pub(super) fn open_multicam_dialog(&mut self, cx: &mut Context<Self>) {
        if self.multicam.is_some() {
            self.close_multicam_dialog(cx);
            return;
        }
        let Some(path) = self.snapshot.path.clone() else {
            self.flash("没有正在播放的文件");
            cx.notify();
            return;
        };
        if self.snapshot.duration <= 0.0 || !self.snapshot.has_video() {
            self.flash("需要先把一个视频文件放起来");
            cx.notify();
            return;
        }

        self.close_remux_dialog(cx);
        self.close_metadata_dialog(cx);
        self.close_proxy_dialog(cx);
        self.close_scope_dialog(cx);
        self.close_scenes_dialog(cx);
        self.close_clip_dialog(cx);
        self.multicam_generation += 1;
        self.multicam = Some(MulticamDialog {
            generation: self.multicam_generation,
            left: path,
            left_duration: self.snapshot.duration,
            right: None,
            right_skip: 0.0,
            state: MulticamState::Idle,
            message: None,
            job: None,
        });
        self.menu = None;
        self.menu_bar = None;
        cx.notify();
    }

    pub(super) fn close_multicam_dialog(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.multicam.take()
            && let Some(job) = &dialog.job
        {
            job.cancel();
        }
        cx.notify();
    }

    pub(super) fn sync_multicam_source(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.multicam.as_ref() else {
            return;
        };
        if self.snapshot.path.as_deref() != Some(dialog.left.as_path()) {
            self.close_multicam_dialog(cx);
        }
    }

    pub(super) fn on_multicam_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if self.multicam.is_none() {
            return false;
        }
        if event.keystroke.key == "escape" {
            self.close_multicam_dialog(cx);
            return true;
        }
        false
    }

    /// 选第二个机位。文件对话框在后台线程里弹，避免卡住渲染。
    fn pick_multicam_right(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut dialog = rfd::FileDialog::new()
            .add_filter("视频文件", formats::VIDEO)
            .add_filter("所有文件", &["*"])
            .set_parent(window);
        if let Some(dir) = self.multicam.as_ref().and_then(|d| d.left.parent()) {
            dialog = dialog.set_directory(dir);
        }
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(dialog.pick_file());
        });
        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                if let Some(path) = picked {
                    this.set_multicam_right(path, cx);
                }
            });
        })
        .detach();
    }

    fn set_multicam_right(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(dialog) = self.multicam.as_mut() else {
            return;
        };
        // 读时长要起一个 ffmpeg，几十毫秒；在这条路径上同步等是值得的——
        // 后面的「能否导出」判断全依赖它，异步反而让按钮状态先错后对。
        match multicam_core::probe_duration(&path) {
            Ok(duration) => {
                dialog.right = Some((path, duration));
                dialog.right_skip = 0.0;
                dialog.message = None;
            }
            Err(error) => {
                dialog.message = Some(format!("{error:#}"));
            }
        }
        cx.notify();
    }

    fn nudge_multicam_skip(&mut self, delta: f64, cx: &mut Context<Self>) {
        let Some(dialog) = self.multicam.as_mut() else {
            return;
        };
        let next = ((dialog.right_skip + delta) * 10.0).round() / 10.0;
        dialog.right_skip = next.clamp(0.0, 600.0);
        cx.notify();
    }

    pub(super) fn start_multicam(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.multicam.as_mut() else {
            return;
        };
        if !matches!(dialog.state, MulticamState::Idle) {
            return;
        }
        let Some((right, _)) = &dialog.right else {
            dialog.message = Some("先选第二个机位".to_string());
            cx.notify();
            return;
        };
        let Some(duration) = dialog.overlap() else {
            dialog.message = Some("两个机位没有重叠：右机位跳过开头太长了".to_string());
            cx.notify();
            return;
        };

        let request = MulticamRequest {
            output: multicam_core::output_path(&dialog.left),
            left: dialog.left.clone(),
            right: right.clone(),
            right_skip: dialog.right_skip,
            duration,
        };
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        match MulticamJob::start(request, tx) {
            Ok(job) => {
                dialog.job = Some(Arc::new(job));
                dialog.state = MulticamState::Running { progress: 0.0 };
                dialog.message = None;
            }
            Err(error) => {
                dialog.message = Some(format!("{error:#}"));
                cx.notify();
                return;
            }
        }
        let generation = dialog.generation;
        cx.notify();

        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let _ = this.update(cx, |this, cx| {
                    let Some(dialog) = this.multicam.as_mut() else {
                        return;
                    };
                    if dialog.generation != generation {
                        return;
                    }
                    match event {
                        MulticamEvent::Progress(progress) => {
                            if let MulticamState::Running { progress: current } = &mut dialog.state
                            {
                                *current = progress;
                            }
                        }
                        MulticamEvent::Done(outcome) => {
                            dialog.job = None;
                            match outcome {
                                MulticamOutcome::Finished(path) => {
                                    dialog.state = MulticamState::Done(path);
                                }
                                MulticamOutcome::Cancelled => {
                                    dialog.state = MulticamState::Idle;
                                }
                                MulticamOutcome::Failed(message) => {
                                    dialog.state = MulticamState::Idle;
                                    dialog.message = Some(message);
                                }
                            }
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn multicam_open_result(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.multicam.as_ref() else {
            return;
        };
        let MulticamState::Done(path) = &dialog.state else {
            return;
        };
        let path = path.clone();
        match platform::open::open(&path) {
            Ok(()) => self.flash(format!("已打开 {}", path.display())),
            Err(message) => self.flash(message),
        }
        cx.notify();
    }

    fn multicam_reveal_result(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.multicam.as_ref() else {
            return;
        };
        let MulticamState::Done(path) = &dialog.state else {
            return;
        };
        let path = path.clone();
        match platform::reveal::reveal(&path) {
            Ok(()) => self.flash("已在文件管理器里定位拼屏文件"),
            Err(message) => self.flash(message),
        }
        cx.notify();
    }

    pub(super) fn render_multicam_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.multicam.as_ref()?;
        let body = match &dialog.state {
            MulticamState::Idle => self.render_multicam_form(dialog, cx),
            MulticamState::Running { progress } => self.render_multicam_progress(*progress, cx),
            MulticamState::Done(path) => self.render_multicam_done(path, cx),
        };
        Some(modal_card(
            "multicam-dialog",
            "双机位并排导出",
            body,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.close_multicam_dialog(cx);
            }),
        ))
    }

    fn render_multicam_form(&self, dialog: &MulticamDialog, cx: &mut Context<Self>) -> AnyElement {
        let overlap = dialog.overlap();
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(camera_row(
                "左机位",
                &file_name(&dialog.left),
                dialog.left_duration,
                None,
            ))
            .child(match &dialog.right {
                Some((path, duration)) => camera_row(
                    "右机位",
                    &file_name(path),
                    *duration,
                    Some(Box::new(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.pick_multicam_right(window, cx);
                    }))),
                ),
                None => div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(text_muted())
                            .child("右机位：未选择"),
                    )
                    .child(settings_button(
                        "multicam-pick",
                        "选择第二个机位…",
                        true,
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.pick_multicam_right(window, cx);
                        }),
                    ))
                    .into_any_element(),
            })
            .child(div().h(px(1.0)).w_full().bg(gpui_kit::rgb(0x2c2c34)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(text())
                                    .child("右机位跳过开头"),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(text_faint())
                                    .child("B 机先开机时，剪掉多出来的几秒"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(settings_button(
                                "multicam-skip-back",
                                "-0.5s",
                                true,
                                cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    this.nudge_multicam_skip(-0.5, cx);
                                }),
                            ))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .w(px(56.0))
                                    .h(px(32.0))
                                    .rounded(px(6.0))
                                    .bg(ui::surface_hover())
                                    .text_size(px(11.0))
                                    .text_color(ui::accent())
                                    .child(format!("{:.1}s", dialog.right_skip)),
                            )
                            .child(settings_button(
                                "multicam-skip-forward",
                                "+0.5s",
                                true,
                                cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    this.nudge_multicam_skip(0.5, cx);
                                }),
                            )),
                    ),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(text_faint())
                    .whitespace_normal()
                    .child(match overlap {
                        Some(duration) => format!(
                            "输出 1280×720 / 30fps / 约 {}；只保留左机位的声音。",
                            format_time(duration)
                        ),
                        None => "两个机位没有重叠，无法导出。".to_string(),
                    }),
            );

        if let Some(message) = &dialog.message {
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
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.0))
                .child(settings_button(
                    "multicam-cancel",
                    "取消",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.close_multicam_dialog(cx);
                    }),
                ))
                .child(primary_button(
                    "multicam-start",
                    "开始导出",
                    overlap.is_some(),
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.start_multicam(cx);
                    }),
                )),
        )
        .into_any_element()
    }

    fn render_multicam_progress(&self, progress: f32, cx: &mut Context<Self>) -> AnyElement {
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
                        "multicam-abort",
                        "取消",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.close_multicam_dialog(cx);
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

    fn render_multicam_done(&self, path: &Path, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(text())
                    .child("拼屏文件已保存"),
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
                        "multicam-reveal",
                        "定位",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.multicam_reveal_result(cx);
                        }),
                    ))
                    .child(settings_button(
                        "multicam-open",
                        "打开",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.multicam_open_result(cx);
                        }),
                    ))
                    .child(primary_button(
                        "multicam-finish",
                        "完成",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.close_multicam_dialog(cx);
                        }),
                    )),
            )
            .into_any_element()
    }
}

/// 「更换」按钮的回调。取个别名，免得参数表被一长串 `dyn Fn` 撑得看不清。
type ReplaceHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// 一行机位信息：名称、时长，以及可选的「更换」按钮。
fn camera_row(
    label: &'static str,
    name: &str,
    duration: f64,
    replace: Option<ReplaceHandler>,
) -> AnyElement {
    let mut row = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(text_muted())
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(text())
                        .child(ui::ellipsis(format!("{name} · {}", format_time(duration)))),
                ),
        );
    if let Some(on_click) = replace {
        row = row.child(settings_button("multicam-replace", "更换", true, on_click));
    }
    row.into_any_element()
}
