use super::super::*;
use super::dialog::{RemuxDialog, RemuxState};
use crate::remux::{
    self as remux_core, RemuxEvent, RemuxJob, RemuxMode, RemuxOutcome, RemuxRequest, TrackAction,
};

impl PlayerApp {
    /// 打开对话框。没有正在播放的文件、或者时长还没读出来时，只给一条提示。
    pub(in crate::app) fn open_remux_dialog(
        &mut self,
        target: EditTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

        // 模态对话框叠在一起会互相挡住，也会抢同一份按键。
        self.close_clip_dialog(cx);
        self.close_metadata_dialog(cx);
        self.close_proxy_dialog(cx);
        self.close_scope_dialog(cx);
        self.close_scenes_dialog(cx);
        self.close_multicam_dialog(cx);
        self.close_playlist_import_dialog(cx);
        self.remux_generation += 1;
        self.remux = Some(RemuxDialog::new(
            self.remux_generation,
            target,
            path,
            &self.snapshot,
            window,
            cx,
        ));
        self.menu = None;
        self.menu_bar = None;
        cx.notify();
    }

    /// 关闭对话框。正在跑的任务一并取消，半成品由任务线程删掉。
    pub(in crate::app) fn close_remux_dialog(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = &self.remux
            && let Some(job) = &dialog.job
        {
            job.cancel();
        }
        self.remux = None;
        cx.notify();
    }

    /// 对话框打开时接管键盘，返回 `true` 表示这次按键已被消费。
    pub(in crate::app) fn on_remux_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
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

    pub(super) fn set_remux_action(&mut self, action: TrackAction, cx: &mut Context<Self>) {
        if let Some(dialog) = self.remux.as_mut() {
            dialog.action = action;
            cx.notify();
        }
    }

    /// 选一个音频 / 字幕文件作为替换来源。
    pub(super) fn pick_remux_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self
            .remux
            .as_ref()
            .filter(|dialog| dialog.editing())
            .map(|dialog| dialog.target)
        else {
            return;
        };
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let dialog = match target {
            EditTarget::Audio => rfd::FileDialog::new().add_filter("音频文件", formats::AUDIO),
            EditTarget::Subtitle => {
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
                    this.set_remux_action(TrackAction::Replace(path), cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 起一个后台任务。命令拼装或进程启动失败时留在编辑态，把原因显示在对话框里。
    pub(in crate::app) fn start_remux(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.remux.as_mut() else {
            return;
        };
        if !dialog.editing() || !dialog.changed(cx) {
            return;
        }
        let tags = match dialog.tags(cx) {
            Ok(tags) => tags,
            Err(message) => {
                dialog.state = RemuxState::Failed(message);
                cx.notify();
                return;
            }
        };
        // 只提交正在编辑的那一路：另一路的语言原样不动，交给 `-map_metadata` 带过去。
        let (audio_tags, subtitle_tags) = match dialog.target {
            EditTarget::Audio => (tags, Vec::new()),
            EditTarget::Subtitle => (Vec::new(), tags),
        };

        let (audio, subtitle) = dialog.actions();
        let request = RemuxRequest {
            mode: RemuxMode::Edit,
            output: remux_core::output_path(&dialog.path, RemuxMode::Edit),
            input: dialog.path.clone(),
            audio_tags,
            subtitle_tags,
            audio,
            subtitle,
            duration: dialog.duration,
            subtitle_codecs: dialog.subtitle_codecs.clone(),
        };
        self.launch_remux(request, cx);
    }

    /// 把某条轨道设成默认轨（同一类里互斥），再点一次取消。
    pub(super) fn toggle_remux_default(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(dialog) = self.remux.as_mut() else {
            return;
        };
        if !dialog.editing() || index >= dialog.rows.len() {
            return;
        }
        let turn_on = !dialog.rows[index].default;
        for (position, row) in dialog.rows.iter_mut().enumerate() {
            row.default = turn_on && position == index;
        }
        cx.notify();
    }

    /// 只导出音轨：不碰画面，第一路音频原样复制。没有内置音轨时按钮是禁用的，
    /// 这里再挡一次，别起一个注定失败的 ffmpeg。
    pub(super) fn extract_audio(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self
            .remux
            .as_ref()
            .filter(|dialog| dialog.editing() && dialog.target == EditTarget::Audio)
        else {
            return;
        };
        if dialog.builtin.is_empty() {
            return;
        }
        let request = RemuxRequest {
            mode: RemuxMode::ExtractAudio,
            output: remux_core::output_path(&dialog.path, RemuxMode::ExtractAudio),
            input: dialog.path.clone(),
            audio_tags: Vec::new(),
            subtitle_tags: Vec::new(),
            audio: TrackAction::Keep,
            subtitle: TrackAction::Remove,
            duration: dialog.duration,
            subtitle_codecs: Vec::new(),
        };
        self.launch_remux(request, cx);
    }

    /// 起后台任务并接管事件。编辑轨道与只导出音轨共用这一套状态机。
    pub(super) fn launch_remux(&mut self, request: RemuxRequest, cx: &mut Context<Self>) {
        let Some(dialog) = self.remux.as_mut() else {
            return;
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

    pub(super) fn remux_open_result(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn remux_reveal_result(&mut self, cx: &mut Context<Self>) {
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
}
