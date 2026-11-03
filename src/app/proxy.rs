//! 生成代理文件的确认 / 进度对话框。
//!
//! 代理是一次性的派生文件，不需要像裁切那样先选区间和参数：点开、确认、
//! 等几秒到几分钟即可。所以这里只有一个按钮和一个进度条。

use super::*;
use crate::proxy::{self as proxy_core, ProxyEvent, ProxyJob, ProxyOutcome};
use std::path::Path;

#[derive(Clone, Debug)]
enum ProxyState {
    Idle,
    Running { progress: f32 },
    Done(PathBuf),
    Failed(String),
}

pub(super) struct ProxyDialog {
    generation: u64,
    path: PathBuf,
    duration: f64,
    state: ProxyState,
    job: Option<Arc<ProxyJob>>,
}

impl PlayerApp {
    pub(super) fn open_proxy_dialog(&mut self, cx: &mut Context<Self>) {
        if self.proxy.is_some() {
            self.close_proxy_dialog(cx);
            return;
        }
        let Some(path) = self.snapshot.path.clone() else {
            self.flash("没有正在播放的文件");
            cx.notify();
            return;
        };
        if self.snapshot.duration <= 0.0 {
            self.flash("还没读出这个文件的时长，稍等一下再试");
            cx.notify();
            return;
        }
        if !self.snapshot.has_video() {
            self.flash("纯音频文件不需要代理");
            cx.notify();
            return;
        }

        // 代理与其它模态 / 面板一样互斥：叠着会抢按键，也会挡住进度条。
        self.close_remux_dialog(cx);
        self.close_metadata_dialog(cx);
        self.close_clip_dialog(cx);
        self.close_scope_dialog(cx);
        self.close_scenes_dialog(cx);
        self.close_multicam_dialog(cx);
        self.proxy_generation += 1;
        self.proxy = Some(ProxyDialog {
            generation: self.proxy_generation,
            path,
            duration: self.snapshot.duration,
            state: ProxyState::Idle,
            job: None,
        });
        self.menu = None;
        self.menu_bar = None;
        cx.notify();
    }

    pub(super) fn close_proxy_dialog(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.proxy.take()
            && let Some(job) = &dialog.job
        {
            job.cancel();
        }
        cx.notify();
    }

    pub(super) fn sync_proxy_source(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.proxy.as_ref() else {
            return;
        };
        if self.snapshot.path.as_deref() != Some(dialog.path.as_path()) {
            self.close_proxy_dialog(cx);
        }
    }

    pub(super) fn on_proxy_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if self.proxy.is_none() {
            return false;
        }
        if event.keystroke.key == "escape" {
            self.close_proxy_dialog(cx);
            return true;
        }
        false
    }

    pub(super) fn start_proxy(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.proxy.as_mut() else {
            return;
        };
        if !matches!(dialog.state, ProxyState::Idle | ProxyState::Failed(_)) {
            return;
        }

        let request = proxy_core::ProxyRequest {
            output: proxy_core::output_path(&dialog.path),
            input: dialog.path.clone(),
            duration: dialog.duration,
        };
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        match ProxyJob::start(request, tx) {
            Ok(job) => {
                dialog.job = Some(Arc::new(job));
                dialog.state = ProxyState::Running { progress: 0.0 };
            }
            Err(error) => {
                dialog.state = ProxyState::Failed(format!("{error:#}"));
                cx.notify();
                return;
            }
        }
        let generation = dialog.generation;
        cx.notify();

        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let _ = this.update(cx, |this, cx| {
                    let Some(dialog) = this.proxy.as_mut() else {
                        return;
                    };
                    if dialog.generation != generation {
                        return;
                    }
                    match event {
                        ProxyEvent::Progress(progress) => {
                            if let ProxyState::Running { progress: current } = &mut dialog.state {
                                *current = progress;
                            }
                        }
                        ProxyEvent::Done(outcome) => {
                            dialog.job = None;
                            dialog.state = match outcome {
                                ProxyOutcome::Finished(path) => ProxyState::Done(path),
                                ProxyOutcome::Cancelled => ProxyState::Idle,
                                ProxyOutcome::Failed(message) => ProxyState::Failed(message),
                            };
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn proxy_open_result(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.proxy.as_ref() else {
            return;
        };
        let ProxyState::Done(path) = &dialog.state else {
            return;
        };
        let path = path.clone();
        match platform::open::open(&path) {
            Ok(()) => self.flash(format!("已打开 {}", path.display())),
            Err(message) => self.flash(message),
        }
        cx.notify();
    }

    fn proxy_reveal_result(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.proxy.as_ref() else {
            return;
        };
        let ProxyState::Done(path) = &dialog.state else {
            return;
        };
        let path = path.clone();
        match platform::reveal::reveal(&path) {
            Ok(()) => self.flash("已在文件管理器里定位代理文件"),
            Err(message) => self.flash(message),
        }
        cx.notify();
    }

    pub(super) fn render_proxy_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.proxy.as_ref()?;
        let body = match &dialog.state {
            ProxyState::Idle | ProxyState::Failed(_) => self.render_proxy_idle(dialog, cx),
            ProxyState::Running { progress } => self.render_proxy_progress(*progress, cx),
            ProxyState::Done(path) => self.render_proxy_done(path, cx),
        };
        Some(modal_card(
            "proxy-dialog",
            "生成代理文件",
            body,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.close_proxy_dialog(cx);
            }),
        ))
    }

    fn render_proxy_idle(&self, dialog: &ProxyDialog, cx: &mut Context<Self>) -> AnyElement {
        let output = proxy_core::output_path(&dialog.path);
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(text_muted())
                    .whitespace_normal()
                    .child(format!(
                        "为「{}」生成 480p 的 H.264 代理，供外部剪辑软件做代理剪辑。",
                        file_name(&dialog.path)
                    )),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_faint())
                    .whitespace_normal()
                    .child(format!("输出：{}", output.display())),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(text_faint())
                    .whitespace_normal()
                    .child("原文件不动；代理不含字幕与章节。生成时间取决于片长与 CPU。"),
            );

        if let ProxyState::Failed(message) = &dialog.state {
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
                    "proxy-cancel",
                    "取消",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.close_proxy_dialog(cx);
                    }),
                ))
                .child(primary_button(
                    "proxy-start",
                    "开始生成",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.start_proxy(cx);
                    }),
                )),
        )
        .into_any_element()
    }

    fn render_proxy_progress(&self, progress: f32, cx: &mut Context<Self>) -> AnyElement {
        let percent = (progress * 100.0).round().clamp(0.0, 100.0) as i32;
        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(text_muted())
                    .child(format!("正在生成代理… {percent}%")),
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
                        "proxy-abort",
                        "取消",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.close_proxy_dialog(cx);
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

    fn render_proxy_done(&self, path: &Path, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(text())
                    .child("代理已生成"),
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
                        "proxy-reveal",
                        "定位",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.proxy_reveal_result(cx);
                        }),
                    ))
                    .child(settings_button(
                        "proxy-open",
                        "打开",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.proxy_open_result(cx);
                        }),
                    ))
                    .child(primary_button(
                        "proxy-finish",
                        "完成",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.close_proxy_dialog(cx);
                        }),
                    )),
            )
            .into_any_element()
    }
}
