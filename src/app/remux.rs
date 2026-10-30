//! 音轨 / 字幕编辑对话框：换成指定文件、整类删掉，或者保持原样。
//!
//! 两件事放在同一个对话框里，因为它们在 ffmpeg 那边是同一次调用：分开做就要把整条
//! 视频流复制两遍。界面结构也完全一样（看当前几条 → 选文件替换 / 删除 / 恢复），
//! 拆成两个对话框只会把同一套排版抄两遍。

use super::*;
use crate::remux::{
    self as remux_core, RemuxEvent, RemuxJob, RemuxOutcome, RemuxRequest, TrackAction,
};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemuxSection {
    Audio,
    Subtitle,
}

/// 对话框当前处在哪一步。
#[derive(Clone, Debug)]
enum RemuxState {
    Editing,
    Running { progress: f32 },
    Done(PathBuf),
    Failed(String),
}

/// 轨道编辑对话框的全部状态。
pub(super) struct RemuxDialog {
    /// 任务代次：后台任务回来时用它确认「这个事件还属于当前对话框」。
    generation: u64,
    path: PathBuf,
    duration: f64,
    /// 原文件内置轨道的摘要，只用来显示「内置几条」。
    audio_tracks: Vec<String>,
    subtitle_tracks: Vec<String>,
    /// 原文件内置字幕轨的编码名，交给 ffmpeg 那边决定保留时怎么处理。
    subtitle_codecs: Vec<String>,
    audio: TrackAction,
    subtitle: TrackAction,
    state: RemuxState,
    job: Option<Arc<RemuxJob>>,
}

impl RemuxDialog {
    fn new(generation: u64, path: PathBuf, snapshot: &Snapshot) -> Self {
        // 外挂字幕不算「内置」：它们不在容器里，ffmpeg 根本看不到，写进摘要只会误导。
        let builtin = |kind: TrackKind| -> Vec<String> {
            snapshot
                .track(kind)
                .into_iter()
                .filter(|track| !track.external)
                .map(|track| track.display_name())
                .collect()
        };
        let subtitle_codecs = snapshot
            .track(TrackKind::Subtitle)
            .into_iter()
            .filter(|track| !track.external)
            .map(|track| track.codec.clone())
            .collect();

        Self {
            generation,
            path,
            duration: snapshot.duration,
            audio_tracks: builtin(TrackKind::Audio),
            subtitle_tracks: builtin(TrackKind::Subtitle),
            subtitle_codecs,
            audio: TrackAction::Keep,
            subtitle: TrackAction::Keep,
            state: RemuxState::Editing,
            job: None,
        }
    }

    fn editing(&self) -> bool {
        matches!(self.state, RemuxState::Editing | RemuxState::Failed(_))
    }

    /// 两项都保持原样时「开始」没有意义：这不是「复制一份文件」的入口。
    fn changed(&self) -> bool {
        !self.audio.is_keep() || !self.subtitle.is_keep()
    }

    fn action(&self, section: RemuxSection) -> &TrackAction {
        match section {
            RemuxSection::Audio => &self.audio,
            RemuxSection::Subtitle => &self.subtitle,
        }
    }

    fn action_mut(&mut self, section: RemuxSection) -> &mut TrackAction {
        match section {
            RemuxSection::Audio => &mut self.audio,
            RemuxSection::Subtitle => &mut self.subtitle,
        }
    }

    fn tracks(&self, section: RemuxSection) -> &[String] {
        match section {
            RemuxSection::Audio => &self.audio_tracks,
            RemuxSection::Subtitle => &self.subtitle_tracks,
        }
    }
}

impl PlayerApp {
    /// 打开对话框。没有正在播放的文件、或者时长还没读出来时，只给一条提示。
    pub(super) fn open_remux_dialog(&mut self, cx: &mut Context<Self>) {
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

        // 两个对话框都是模态的，叠在一起会互相挡住，也会抢同一份按键。
        self.close_clip_dialog(cx);
        self.remux_generation += 1;
        self.remux = Some(RemuxDialog::new(
            self.remux_generation,
            path,
            &self.snapshot,
        ));
        self.menu = None;
        self.menu_bar = None;
        cx.notify();
    }

    /// 关闭对话框。正在跑的任务一并取消，半成品由任务线程删掉。
    pub(super) fn close_remux_dialog(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = &self.remux
            && let Some(job) = &dialog.job
        {
            job.cancel();
        }
        self.remux = None;
        cx.notify();
    }

    /// 对话框打开时接管键盘，返回 `true` 表示这次按键已被消费。
    pub(super) fn on_remux_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(dialog) = self.remux.as_ref() else {
            return false;
        };

        if event.keystroke.key == "escape" {
            self.close_remux_dialog(cx);
            return true;
        }
        // 运行中除 Esc 外一律吞掉：空格、方向键这些是播放快捷键，不能漏下去。
        if !dialog.editing() {
            return true;
        }
        if event.keystroke.key == "enter" {
            self.start_remux(cx);
        }
        true
    }

    fn set_remux_action(
        &mut self,
        section: RemuxSection,
        action: TrackAction,
        cx: &mut Context<Self>,
    ) {
        if let Some(dialog) = self.remux.as_mut() {
            *dialog.action_mut(section) = action;
            cx.notify();
        }
    }

    /// 选一个音频 / 字幕文件作为替换来源。
    fn pick_remux_source(
        &mut self,
        section: RemuxSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog_open || !self.remux.as_ref().is_some_and(RemuxDialog::editing) {
            return;
        }
        self.dialog_open = true;

        let dialog = match section {
            RemuxSection::Audio => rfd::FileDialog::new().add_filter("音频文件", formats::AUDIO),
            RemuxSection::Subtitle => {
                rfd::FileDialog::new().add_filter("字幕文件", formats::SUBTITLE)
            }
        };
        let mut dialog = dialog.add_filter("所有文件", &["*"]).set_parent(window);
        // 从当前片子所在目录开始翻：替换的音轨/字幕通常就放在它旁边。
        if let Some(dir) = self.remux.as_ref().and_then(|dialog| dialog.path.parent()) {
            dialog = dialog.set_directory(dir);
        }

        // rfd 的对话框是阻塞的：扔到独立线程，结果通过 oneshot 回来。
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(dialog.pick_file());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if let Some(path) = picked {
                    this.set_remux_action(section, TrackAction::Replace(path), cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 起一个后台任务。命令拼装或进程启动失败时留在编辑态，把原因显示在对话框里。
    pub(super) fn start_remux(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.remux.as_mut() else {
            return;
        };
        if !dialog.editing() || !dialog.changed() {
            return;
        }

        let request = RemuxRequest {
            output: remux_core::output_path(&dialog.path),
            input: dialog.path.clone(),
            audio: dialog.audio.clone(),
            subtitle: dialog.subtitle.clone(),
            duration: dialog.duration,
            subtitle_codecs: dialog.subtitle_codecs.clone(),
        };
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        match RemuxJob::start(request, tx) {
            Ok(job) => {
                dialog.job = Some(Arc::new(job));
                dialog.state = RemuxState::Running { progress: 0.0 };
            }
            // 进程都没起来，不会有任何事件，别去等一个空通道。
            Err(error) => {
                dialog.state = RemuxState::Failed(format!("{error:#}"));
                cx.notify();
                return;
            }
        }
        let generation = dialog.generation;
        cx.notify();

        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let _ = this.update(cx, |this, cx| {
                    let Some(dialog) = this.remux.as_mut() else {
                        return;
                    };
                    // 旧任务可能在新对话框打开之后才收尾：只认当前代次。
                    if dialog.generation != generation {
                        return;
                    }
                    match event {
                        RemuxEvent::Progress(progress) => {
                            if let RemuxState::Running { progress: current } = &mut dialog.state {
                                *current = progress;
                            }
                        }
                        RemuxEvent::Done(outcome) => {
                            dialog.job = None;
                            dialog.state = match outcome {
                                RemuxOutcome::Finished(path) => RemuxState::Done(path),
                                RemuxOutcome::Cancelled => RemuxState::Editing,
                                RemuxOutcome::Failed(message) => RemuxState::Failed(message),
                            };
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn remux_open_result(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.remux.as_ref() else {
            return;
        };
        let RemuxState::Done(path) = &dialog.state else {
            return;
        };
        match platform::open::open(path) {
            Ok(()) => {
                let path = path.clone();
                self.flash(format!("已打开 {}", path.display()));
            }
            Err(message) => self.flash(message),
        }
        cx.notify();
    }

    fn remux_reveal_result(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.remux.as_ref() else {
            return;
        };
        let RemuxState::Done(path) = &dialog.state else {
            return;
        };
        let path = path.clone();
        match platform::reveal::reveal(&path) {
            Ok(()) => self.flash("已在文件管理器里定位新文件"),
            Err(message) => self.flash(message),
        }
        cx.notify();
    }

    /// 对话框本体。放在模态层最上面，遮住底下的视频与菜单。
    pub(super) fn render_remux_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.remux.as_ref()?;
        let body = match &dialog.state {
            RemuxState::Editing | RemuxState::Failed(_) => self.render_remux_editor(dialog, cx),
            RemuxState::Running { progress } => self.render_remux_progress(*progress, cx),
            RemuxState::Done(path) => self.render_remux_done(path, cx),
        };

        Some(modal_card(
            "remux-dialog",
            "编辑音轨与字幕",
            body,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.close_remux_dialog(cx);
            }),
        ))
    }

    fn render_remux_editor(&self, dialog: &RemuxDialog, cx: &mut Context<Self>) -> AnyElement {
        let name = file_name(&dialog.path);
        let mut body = div().flex().flex_col().gap(px(12.0)).child(
            div()
                .text_size(px(12.0))
                .text_color(text_muted())
                .child(ui::ellipsis(format!(
                    "{name} · 全长 {}",
                    format_time(dialog.duration)
                ))),
        );

        // 失败原因来自 ffmpeg，可能有多行；单独一块红字，别和按钮混在一起。
        if let RemuxState::Failed(message) = &dialog.state {
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

        body = body
            .child(self.render_remux_section(RemuxSection::Audio, dialog, cx))
            .child(self.render_remux_section(RemuxSection::Subtitle, dialog, cx))
            // 横线：卡片是竖排的，`card_divider` 那条竖线在这里会变成一个小点。
            .child(div().h(px(1.0)).w_full().bg(gpui_kit::rgb(0x2c2c34)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.0))
                    .child(settings_button(
                        "remux-cancel",
                        "取消",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.close_remux_dialog(cx);
                        }),
                    ))
                    .child(primary_button(
                        "remux-start",
                        "开始编辑",
                        dialog.changed(),
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.start_remux(cx);
                        }),
                    )),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(text_faint())
                    .whitespace_normal()
                    .child(
                        "输出为同目录下的新 mkv：画面与保留的轨道直接复制，不重新编码；\
                         换上的字幕会转成容器支持的编码。原文件不会被改动。",
                    ),
            );
        body.into_any_element()
    }

    /// 一组轨道：标题 + 当前处置 + 三个按钮。音频与字幕共用，只有文案不同。
    fn render_remux_section(
        &self,
        section: RemuxSection,
        dialog: &RemuxDialog,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (title, pick_label, remove_label, keep_label, pick_id, remove_id, keep_id) =
            match section {
                RemuxSection::Audio => (
                    "音频",
                    "选择音频文件…",
                    "删除音轨",
                    "恢复原音轨",
                    "remux-audio-pick",
                    "remux-audio-remove",
                    "remux-audio-keep",
                ),
                RemuxSection::Subtitle => (
                    "字幕",
                    "选择字幕文件…",
                    "移除字幕",
                    "恢复原字幕",
                    "remux-subtitle-pick",
                    "remux-subtitle-remove",
                    "remux-subtitle-keep",
                ),
            };

        let builtin = dialog.tracks(section);
        let action = dialog.action(section);
        let (status, color) = match action {
            TrackAction::Keep if builtin.is_empty() => ("没有内置轨道".to_string(), text_faint()),
            TrackAction::Keep => (
                format!("内置 {} 条：{}", builtin.len(), builtin.join("、")),
                text_muted(),
            ),
            TrackAction::Replace(path) => (format!("将替换为：{}", file_name(path)), ui::accent()),
            TrackAction::Remove => ("将删除全部同类轨道".to_string(), ui::danger()),
        };

        let pick = cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.pick_remux_source(section, window, cx);
        });
        let remove = cx.listener(move |this, _: &ClickEvent, _window, cx| {
            this.set_remux_action(section, TrackAction::Remove, cx);
        });
        let keep = cx.listener(move |this, _: &ClickEvent, _window, cx| {
            this.set_remux_action(section, TrackAction::Keep, cx);
        });

        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.0))
                            .text_color(text_muted())
                            .child(title),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(12.0))
                            .text_color(color)
                            .child(ui::ellipsis(status)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(settings_button(pick_id, pick_label, true, pick))
                    .child(settings_button(
                        remove_id,
                        remove_label,
                        !matches!(action, TrackAction::Remove),
                        remove,
                    ))
                    // 「恢复」只在改过之后才有意义，没改过时多一个按钮只是噪音。
                    .when(!action.is_keep(), |this| {
                        this.child(settings_button(keep_id, keep_label, true, keep))
                    }),
            )
            .into_any_element()
    }

    fn render_remux_progress(&self, progress: f32, cx: &mut Context<Self>) -> AnyElement {
        let percent = (progress * 100.0).round().clamp(0.0, 100.0) as i32;
        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(text_muted())
                    .child(format!("正在写入新文件… {percent}%")),
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
                        "remux-abort",
                        "取消",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            // 关闭即取消：任务线程负责杀掉 ffmpeg 并删掉半成品。
                            this.close_remux_dialog(cx);
                        }),
                    )),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(text_faint())
                    .child("编辑在后台进行，关掉对话框或按 Esc 会取消并删除半成品。"),
            )
            .into_any_element()
    }

    fn render_remux_done(&self, path: &Path, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(text())
                    .child("新文件已保存"),
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
                        "remux-reveal",
                        "定位",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.remux_reveal_result(cx);
                        }),
                    ))
                    .child(settings_button(
                        "remux-open",
                        "打开",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.remux_open_result(cx);
                        }),
                    ))
                    .child(primary_button(
                        "remux-finish",
                        "完成",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.close_remux_dialog(cx);
                        }),
                    )),
            )
            .into_any_element()
    }
}
