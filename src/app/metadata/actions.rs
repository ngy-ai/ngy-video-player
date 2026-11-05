use super::super::*;
use super::dialog::{ChapterRow, MetadataDialog, MetadataSource, MetadataState};
use crate::clip::{self as clip_core, ParsedTime};
use crate::metadata::{self as metadata_core, MetadataEvent, MetadataJob, MetadataOutcome, Tags};
use gpui_kit::component::input::InputState;

impl PlayerApp {
    /// 打开对话框。没有正在播放的文件、或者时长还没读出来时，只给一条提示。
    pub(in crate::app) fn open_metadata_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.metadata.is_some() {
            self.close_metadata_dialog(cx);
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

        // 与轨道编辑一样是模态的：叠着会互相挡住，也会抢同一份按键。
        self.close_remux_dialog(cx);
        self.close_clip_dialog(cx);
        self.close_proxy_dialog(cx);
        self.close_scope_dialog(cx);
        self.close_scenes_dialog(cx);
        self.close_multicam_dialog(cx);
        self.close_playlist_import_dialog(cx);
        self.metadata_generation += 1;

        let chapters = self
            .snapshot
            .chapters
            .iter()
            .map(|chapter| (chapter.title.clone(), chapter.time))
            .collect();
        let subtitle_codecs = self
            .snapshot
            .track(TrackKind::Subtitle)
            .into_iter()
            .filter(|track| !track.external)
            .map(|track| track.codec.clone())
            .collect();

        // 快照里的类型只读过一次，换成编辑模块自己的结构再交给对话框。
        let tags = Tags {
            title: self.snapshot.tags.title.clone(),
            artist: self.snapshot.tags.artist.clone(),
            comment: self.snapshot.tags.comment.clone(),
        };
        let dialog = MetadataDialog::new(
            self.metadata_generation,
            MetadataSource {
                path,
                duration: self.snapshot.duration,
                fps: self.snapshot.fps,
                tags: &tags,
                chapters,
                subtitle_codecs,
            },
            window,
            cx,
        );
        // 打开就落在标题框上：最常见的改动就是改标题。
        dialog.title.update(cx, |state, cx| state.focus(window, cx));
        self.metadata = Some(dialog);
        self.menu = None;
        self.menu_bar = None;
        cx.notify();
    }

    /// 关闭对话框。正在跑的任务一并取消，半成品由任务线程删掉。
    pub(in crate::app) fn close_metadata_dialog(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.metadata.take()
            && let Some(job) = &dialog.job
        {
            job.cancel();
        }
        cx.notify();
    }

    /// 换片后旧表单不能再留：标签与章节都属于上一个文件。
    pub(in crate::app) fn sync_metadata_source(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.metadata.as_ref() else {
            return;
        };
        if self.snapshot.path.as_deref() != Some(dialog.path.as_path()) {
            self.close_metadata_dialog(cx);
        }
    }

    /// 对话框打开时只接管 Esc；文字输入交给获得焦点的 Input。
    pub(in crate::app) fn on_metadata_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.metadata.is_none() {
            return false;
        }
        if event.keystroke.key == "escape" {
            self.close_metadata_dialog(cx);
            return true;
        }
        false
    }

    /// 以当前播放位置添加一条章节。
    pub(super) fn add_metadata_chapter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let position = self
            .snapshot
            .position
            .clamp(0.0, self.snapshot.duration.max(0.0));
        let Some(dialog) = self.metadata.as_mut() else {
            return;
        };
        if !dialog.editing() {
            return;
        }
        let duplicate = dialog.chapters.iter().any(|row| {
            matches!(
                clip_core::parse_time_with_fps(&row.start.read(cx).value(), dialog.fps),
                ParsedTime::Value(start) if (start - position).abs() < 0.1
            )
        });
        if duplicate {
            self.flash("这个位置已经有章节了");
            cx.notify();
            return;
        }

        let start_text = clip_core::format_time(position);
        let start = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(start_text)
                .placeholder("00:00.000")
        });
        let title = format!("章节 {}", dialog.chapters.len() + 1);
        let title = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(title)
                .placeholder("章节标题")
        });
        title.update(cx, |state, cx| state.focus(window, cx));
        dialog.chapters.push(ChapterRow { start, title });
        // 按当前时间码排一下：用户可能先改过某一条的时间，再添加新的一条。
        // 解析失败的条目排到最后，保存时的校验会把它挑出来。
        let fps = dialog.fps;
        dialog.chapters.sort_by_key(|row| {
            match clip_core::parse_time_with_fps(&row.start.read(cx).value(), fps) {
                ParsedTime::Value(value) => (0, (value * 1000.0).round() as i64),
                _ => (1, 0),
            }
        });
        cx.notify();
    }

    pub(super) fn remove_metadata_chapter(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(dialog) = self.metadata.as_mut() else {
            return;
        };
        if index < dialog.chapters.len() {
            dialog.chapters.remove(index);
            cx.notify();
        }
    }

    /// 选一张图片当封面。文件对话框是阻塞的，扔到独立线程，结果走 oneshot 回来。
    pub(super) fn pick_metadata_cover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.metadata.as_ref().filter(|dialog| dialog.editing()) else {
            return;
        };
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let mut picker = rfd::FileDialog::new()
            .add_filter("图片", formats::IMAGE)
            .add_filter("所有文件", &["*"])
            .set_parent(window);
        // 从片子所在目录开始翻：封面通常就放在它旁边。
        if let Some(dir) = dialog.path.parent() {
            picker = picker.set_directory(dir);
        }

        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(picker.pick_file());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if let Some(path) = picked
                    && let Some(dialog) = this.metadata.as_mut()
                {
                    dialog.cover = Some(path);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn clear_metadata_cover(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.metadata.as_mut() {
            dialog.cover = None;
            cx.notify();
        }
    }

    /// 起一个后台任务。命令拼装或进程启动失败时留在编辑态，把原因显示在对话框里。
    pub(in crate::app) fn start_metadata(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.metadata.as_mut() else {
            return;
        };
        if !dialog.editing() {
            return;
        }

        let chapters = match dialog.chapter_edits(cx, dialog.duration) {
            Ok(chapters) => metadata_core::normalize_chapters(chapters, dialog.duration),
            Err(message) => {
                dialog.state = MetadataState::Failed(message);
                cx.notify();
                return;
            }
        };
        let request = crate::metadata::MetadataRequest {
            output: metadata_core::output_path(&dialog.path),
            input: dialog.path.clone(),
            duration: dialog.duration,
            tags: dialog.tags(cx),
            chapters,
            subtitle_codecs: dialog.subtitle_codecs.clone(),
            cover: dialog.cover.clone(),
        };
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        match MetadataJob::start(request, tx) {
            Ok(job) => {
                dialog.job = Some(Arc::new(job));
                dialog.state = MetadataState::Running { progress: 0.0 };
            }
            Err(error) => {
                dialog.state = MetadataState::Failed(format!("{error:#}"));
                cx.notify();
                return;
            }
        }
        let generation = dialog.generation;
        cx.notify();

        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let _ = this.update(cx, |this, cx| {
                    let Some(dialog) = this.metadata.as_mut() else {
                        return;
                    };
                    if dialog.generation != generation {
                        return;
                    }
                    match event {
                        MetadataEvent::Progress(progress) => {
                            if let MetadataState::Running { progress: current } = &mut dialog.state
                            {
                                *current = progress;
                            }
                        }
                        MetadataEvent::Done(outcome) => {
                            dialog.job = None;
                            dialog.state = match outcome {
                                MetadataOutcome::Finished(path) => MetadataState::Done(path),
                                MetadataOutcome::Cancelled => MetadataState::Editing,
                                MetadataOutcome::Failed(message) => MetadataState::Failed(message),
                            };
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(super) fn metadata_open_result(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.metadata.as_ref() else {
            return;
        };
        let MetadataState::Done(path) = &dialog.state else {
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

    pub(super) fn metadata_reveal_result(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.metadata.as_ref() else {
            return;
        };
        let MetadataState::Done(path) = &dialog.state else {
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
