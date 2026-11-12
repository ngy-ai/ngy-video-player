//! 生成代理文件的确认 / 进度对话框。
//!
//! 代理是一次性的派生文件，不需要像裁切那样先选区间和参数：点开、确认、
//! 等几秒到几分钟即可。所以这里只有一个按钮和一个进度条。

use super::*;
use crate::proxy::{self as proxy_core, ProxyEvent, ProxyJob, ProxyOutcome, ProxyQuality};
use std::path::Path;

/// 档位 chip 的元素 id（`chip` 要 `&'static str`，档位是编译期就定下的三档）。
fn quality_chip_id(quality: ProxyQuality) -> &'static str {
    match quality {
        ProxyQuality::P480 => "proxy-quality-480",
        ProxyQuality::P720 => "proxy-quality-720",
        ProxyQuality::P1080 => "proxy-quality-1080",
    }
}

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
    quality: ProxyQuality,
    /// 已经有代理时写回同一个文件（换档重生成不该攒出一堆 `(2)`）。
    overwrite: Option<PathBuf>,
    /// 是从「正在播放的文件」进来的：换片就关掉对话框。
    ///
    /// 从素材库进来的素材与当前播放的文件无关，换片不该把它关掉。
    follow_playing: bool,
    state: ProxyState,
    job: Option<Arc<ProxyJob>>,
}

impl ProxyDialog {
    /// 这次要写到哪个文件。
    fn output(&self) -> PathBuf {
        self.overwrite
            .clone()
            .unwrap_or_else(|| proxy_core::output_path(&self.path))
    }
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
        self.open_proxy_dialog_at(path, self.snapshot.duration, true, cx);
    }

    /// 给素材库里的某个素材生成代理：时长与有没有画面都来自探测结果。
    ///
    /// 代理丢了（文件被删 / 挪走）也走这里重新生成——落点还是同一个文件名规则。
    pub(super) fn open_proxy_dialog_for_asset(&mut self, id: AssetId, cx: &mut Context<Self>) {
        if self.proxy.is_some() {
            self.close_proxy_dialog(cx);
            return;
        }
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let Some(asset) = project.asset(id) else {
            return;
        };
        let Some(media) = asset.media.as_ref() else {
            self.flash("这个素材还没探测完，稍等一下再试");
            cx.notify();
            return;
        };
        if !media.has_video() {
            self.flash("纯音频素材不需要代理");
            cx.notify();
            return;
        }
        let (path, duration) = (asset.resolved.clone(), media.duration.secs());
        if crate::timeline::is_proxy_file(&path) {
            self.flash("这个素材本身就是代理文件，不需要再生成");
            cx.notify();
            return;
        }
        self.open_proxy_dialog_at(path, duration, false, cx);
    }

    fn open_proxy_dialog_at(
        &mut self,
        path: PathBuf,
        duration: f64,
        follow_playing: bool,
        cx: &mut Context<Self>,
    ) {
        // 代理与其它模态 / 面板一样互斥：叠着会抢按键，也会挡住进度条。
        self.close_remux_dialog(cx);
        self.close_metadata_dialog(cx);
        self.close_clip_dialog(cx);
        self.close_scope_dialog(cx);
        self.close_scenes_dialog(cx);
        self.close_multicam_dialog(cx);
        self.close_playlist_import_dialog(cx);
        // 已经有代理就原地覆盖：换档重生成、或者代理被挪走后又放回来，都不该攒 `(2)`。
        // 输入本身是代理文件时不覆盖——那是用户自己的素材，不能当派生文件盖掉。
        let overwrite = if crate::timeline::is_proxy_file(&path) {
            None
        } else {
            crate::timeline::find_proxy(&path, |candidate| candidate.exists())
        };
        self.proxy_generation += 1;
        self.proxy = Some(ProxyDialog {
            generation: self.proxy_generation,
            path,
            duration,
            quality: ProxyQuality::default(),
            overwrite,
            follow_playing,
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
        // 只有从「正在播放的文件」进来的才跟着换片关掉：素材库那条与播放的文件无关。
        if dialog.follow_playing && self.snapshot.path.as_deref() != Some(dialog.path.as_path()) {
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
        // 先把要用的参数抄出来，`dialog` 的可变借用不能跨到 `detach_preview` 那边去。
        let Some(dialog) = self.proxy.as_ref() else {
            return;
        };
        if !matches!(dialog.state, ProxyState::Idle | ProxyState::Failed(_)) {
            return;
        }
        let request = proxy_core::ProxyRequest {
            output: dialog.output(),
            input: dialog.path.clone(),
            duration: dialog.duration,
            quality: dialog.quality,
        };
        let generation = dialog.generation;

        // 覆盖写之前先把预览从这个文件上撤下来：Windows 上文件被 mpv 读着的时候
        // ffmpeg 打不开目标文件写入，会以「Permission denied」告终。
        self.detach_preview(&request.output);

        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let started = ProxyJob::start(request, tx);
        let Some(dialog) = self.proxy.as_mut() else {
            return;
        };
        match started {
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
        let output = dialog.output();
        let replacing = dialog.overwrite.is_some();
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
                        "为「{}」生成 {} 的 H.264 代理，供剪辑时做代理预览。",
                        file_name(&dialog.path),
                        dialog.quality.label()
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text_faint())
                            .child("规格"),
                    )
                    .children(ProxyQuality::ALL.into_iter().map(|quality| {
                        let active = dialog.quality == quality;
                        chip(
                            quality_chip_id(quality),
                            quality.label(),
                            active,
                            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                if let Some(dialog) = this.proxy.as_mut() {
                                    dialog.quality = quality;
                                }
                                cx.notify();
                            }),
                        )
                    })),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_faint())
                    .whitespace_normal()
                    .child(format!(
                        "输出：{}{}",
                        output.display(),
                        if replacing {
                            "（已有代理，会被覆盖）"
                        } else {
                            ""
                        }
                    )),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(text_faint())
                    .whitespace_normal()
                    .child("原文件不动；代理不含字幕与章节。档位越低越小越快，导出始终用原片。"),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog(input: PathBuf, overwrite: Option<PathBuf>) -> ProxyDialog {
        ProxyDialog {
            generation: 1,
            path: input,
            duration: 60.0,
            quality: ProxyQuality::P480,
            overwrite,
            follow_playing: false,
            state: ProxyState::Idle,
            job: None,
        }
    }

    #[test]
    fn 已有代理时原地覆盖() {
        // 目录不建出来：`output_path` 只做存在性判断，落点一定是无编号的那个。
        let dir = std::env::temp_dir().join(format!("ngy-proxy-dialog-{}", std::process::id()));
        let input = dir.join("a.mov");
        let expected = dir.join("a.proxy.mp4");

        // 已经有代理：写到原来那个文件，不攒 `(2)`。
        let existing = dialog(input.clone(), Some(expected.clone()));
        assert_eq!(existing.output(), expected);
        // 代理丢了（或从来没生成过）：按命名规则算落点。
        let fresh = dialog(input, None);
        assert_eq!(fresh.output(), expected);
    }
}
