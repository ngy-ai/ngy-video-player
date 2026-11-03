//! 章节与元数据编辑对话框：改标题 / 作者 / 备注，增删章节。
//!
//! 输入框用组件库的 [`Input`] 而不是自绘：这里要输中文，自绘输入框没有输入法
//! 与选区（裁切面板的时间框只收数字，可以省掉这一层）。代价是样式来自组件库
//! 主题，跟播放器其余部分略有差别——编辑标题比样式统一更重要。
//!
//! 保存时画面、音轨、字幕、附件全部流复制，只重写容器头，所以几秒结束；输出
//! 到原文件同目录的新 mkv，原文件不动。

use super::*;
use crate::clip::{self as clip_core, ParsedTime};
use crate::metadata::{
    self as metadata_core, ChapterEdit, MetadataEvent, MetadataJob, MetadataOutcome, Tags,
};
use gpui_kit::component::input::{Input, InputState};
use std::path::Path;

/// 对话框当前处在哪一步。
#[derive(Clone, Debug)]
enum MetadataState {
    Editing,
    Running { progress: f32 },
    Done(PathBuf),
    Failed(String),
}

/// 一条可编辑的章节。
struct ChapterRow {
    /// 起点文本，接受 `90`、`1:30`、`01:02:03.500`；保存时才解析。
    /// 用输入框而不是只读文本，是为了能直接改时间码修章节。
    start: Entity<InputState>,
    title: Entity<InputState>,
}

/// 打开对话框时从播放器快照里抄出来的一份文件信息。
///
/// 单独成结构是为了让 `MetadataDialog::new` 只收两个参数：数据一批、窗口与上下文一批。
struct MetadataSource<'a> {
    path: PathBuf,
    duration: f64,
    /// 片源帧率，用来换算 SMPTE 时间码里的帧号。0 表示还没读出来。
    fps: f64,
    tags: &'a Tags,
    chapters: Vec<(String, f64)>,
    /// 原文件内置字幕的编码名，保存时决定字幕是复制还是转码。
    subtitle_codecs: Vec<String>,
}

/// 对话框的全部状态。
pub(super) struct MetadataDialog {
    generation: u64,
    path: PathBuf,
    duration: f64,
    /// 片源帧率，用来换算 SMPTE 时间码里的帧号。
    fps: f64,
    title: Entity<InputState>,
    artist: Entity<InputState>,
    comment: Entity<InputState>,
    chapters: Vec<ChapterRow>,
    /// 原文件内置字幕的编码名，保存时决定字幕是复制还是转码。
    subtitle_codecs: Vec<String>,
    /// 新选的封面图。`None` 表示沿用原文件的封面（原文件没有就不写）。
    cover: Option<PathBuf>,
    state: MetadataState,
    job: Option<Arc<MetadataJob>>,
}

impl MetadataDialog {
    fn new(
        generation: u64,
        source: MetadataSource<'_>,
        window: &mut Window,
        cx: &mut Context<PlayerApp>,
    ) -> Self {
        let title = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(source.tags.title.clone())
                .placeholder("留空则删除标题")
        });
        let artist = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(source.tags.artist.clone())
                .placeholder("留空则删除作者")
        });
        let comment = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(source.tags.comment.clone())
                .placeholder("留空则删除备注")
        });

        let mut chapters: Vec<(String, f64)> = source.chapters;
        chapters.sort_by(|a, b| a.1.total_cmp(&b.1));
        let chapters = chapters
            .into_iter()
            .map(|(title, start)| {
                let start_text = clip_core::format_time(start);
                ChapterRow {
                    start: cx.new(|cx| {
                        InputState::new(window, cx)
                            .default_value(start_text)
                            .placeholder("00:00.000")
                    }),
                    title: cx.new(|cx| {
                        InputState::new(window, cx)
                            .default_value(title)
                            .placeholder("章节标题")
                    }),
                }
            })
            .collect();

        Self {
            generation,
            path: source.path,
            duration: source.duration,
            fps: source.fps,
            title,
            artist,
            comment,
            chapters,
            subtitle_codecs: source.subtitle_codecs,
            cover: None,
            state: MetadataState::Editing,
            job: None,
        }
    }

    fn editing(&self) -> bool {
        matches!(
            self.state,
            MetadataState::Editing | MetadataState::Failed(_)
        )
    }

    /// 当前表单里的标签。空字符串表示删除该标签。
    fn tags(&self, cx: &App) -> Tags {
        Tags {
            title: self.title.read(cx).value().trim().to_string(),
            artist: self.artist.read(cx).value().trim().to_string(),
            comment: self.comment.read(cx).value().trim().to_string(),
        }
    }

    /// 解析所有时间框。任何一条不合法都整体拒绝：带着错的章节去写文件，
    /// 用户会得到一个「保存成功但章节不对」的结果，比直接报错更难查。
    fn chapter_edits(&self, cx: &App, duration: f64) -> Result<Vec<ChapterEdit>, String> {
        self.chapters
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let text = row.start.read(cx).value();
                let start = match clip_core::parse_time_with_fps(&text, self.fps) {
                    ParsedTime::Value(start) if start < duration => start,
                    ParsedTime::Value(_) => {
                        return Err(format!(
                            "第 {} 条章节超出片长（{}）",
                            index + 1,
                            clip_core::format_time(duration)
                        ));
                    }
                    ParsedTime::Empty => {
                        return Err(format!("第 {} 条章节的时间是空的", index + 1));
                    }
                    ParsedTime::Invalid => {
                        return Err(format!(
                            "第 {} 条章节的时间格式不对：{}（可写 90、1:30、01:02:03.500、01:02:03;15）",
                            index + 1,
                            text
                        ));
                    }
                };
                Ok(ChapterEdit {
                    title: row.title.read(cx).value().trim().to_string(),
                    start,
                })
            })
            .collect()
    }
}

impl PlayerApp {
    /// 打开对话框。没有正在播放的文件、或者时长还没读出来时，只给一条提示。
    pub(super) fn open_metadata_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        self.close_multicam_dialog(cx);
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
    pub(super) fn close_metadata_dialog(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.metadata.take()
            && let Some(job) = &dialog.job
        {
            job.cancel();
        }
        cx.notify();
    }

    /// 换片后旧表单不能再留：标签与章节都属于上一个文件。
    pub(super) fn sync_metadata_source(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.metadata.as_ref() else {
            return;
        };
        if self.snapshot.path.as_deref() != Some(dialog.path.as_path()) {
            self.close_metadata_dialog(cx);
        }
    }

    /// 对话框打开时只接管 Esc；文字输入交给获得焦点的 Input。
    pub(super) fn on_metadata_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
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
    fn add_metadata_chapter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn remove_metadata_chapter(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(dialog) = self.metadata.as_mut() else {
            return;
        };
        if index < dialog.chapters.len() {
            dialog.chapters.remove(index);
            cx.notify();
        }
    }

    /// 选一张图片当封面。文件对话框是阻塞的，扔到独立线程，结果走 oneshot 回来。
    fn pick_metadata_cover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn clear_metadata_cover(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.metadata.as_mut() {
            dialog.cover = None;
            cx.notify();
        }
    }

    /// 起一个后台任务。命令拼装或进程启动失败时留在编辑态，把原因显示在对话框里。
    pub(super) fn start_metadata(&mut self, cx: &mut Context<Self>) {
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

    fn metadata_open_result(&mut self, cx: &mut Context<Self>) {
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

    fn metadata_reveal_result(&mut self, cx: &mut Context<Self>) {
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

    /// 对话框本体，放在模态层。
    pub(super) fn render_metadata_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.metadata.as_ref()?;
        let body = match &dialog.state {
            MetadataState::Editing | MetadataState::Failed(_) => {
                self.render_metadata_editor(dialog, cx)
            }
            MetadataState::Running { progress } => self.render_metadata_progress(*progress, cx),
            MetadataState::Done(path) => self.render_metadata_done(path, cx),
        };
        Some(modal_card(
            "metadata-dialog",
            "章节与元数据",
            body,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.close_metadata_dialog(cx);
            }),
        ))
    }

    fn render_metadata_editor(
        &self,
        dialog: &MetadataDialog,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut body = div().flex().flex_col().gap(px(12.0)).child(
            div()
                .text_size(px(12.0))
                .text_color(text_muted())
                .child(ui::ellipsis(format!(
                    "{} · 全长 {}",
                    file_name(&dialog.path),
                    format_time(dialog.duration)
                ))),
        );

        if let MetadataState::Failed(message) = &dialog.state {
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
            .child(tag_field("metadata-title", "标题", &dialog.title))
            .child(tag_field("metadata-artist", "作者", &dialog.artist))
            .child(tag_field("metadata-comment", "备注", &dialog.comment))
            .child(self.render_metadata_cover(dialog, cx))
            .child(div().h(px(1.0)).w_full().bg(gpui_kit::rgb(0x2c2c34)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(text_muted())
                            .child(format!("章节（{}）", dialog.chapters.len())),
                    )
                    .child(settings_button(
                        "metadata-add-chapter",
                        "在当前位置添加",
                        true,
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.add_metadata_chapter(window, cx);
                        }),
                    )),
            );

        if dialog.chapters.is_empty() {
            body = body.child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_faint())
                    .child("没有章节。播到要标记的位置，点「在当前位置添加」。"),
            );
        } else {
            let mut list = div()
                .id("metadata-chapter-list")
                .flex()
                .flex_col()
                .gap(px(6.0))
                .max_h(px(240.0))
                .overflow_y_scroll();
            for (index, row) in dialog.chapters.iter().enumerate() {
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(
                            div().w(px(118.0)).flex_none().child(
                                Input::new(&row.start)
                                    .id(SharedString::from(format!(
                                        "metadata-chapter-start-{index}"
                                    )))
                                    .w_full(),
                            ),
                        )
                        .child(
                            div().flex_1().min_w(px(0.0)).child(
                                Input::new(&row.title)
                                    .id(SharedString::from(format!("metadata-chapter-{index}")))
                                    .w_full(),
                            ),
                        )
                        .child(icon_button(
                            SharedString::from(format!("metadata-chapter-remove-{index}")),
                            IconName::X,
                            false,
                            false,
                            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                this.remove_metadata_chapter(index, cx);
                            }),
                        )),
                );
            }
            body = body.child(list);
        }

        body.child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.0))
                .child(settings_button(
                    "metadata-cancel",
                    "取消",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.close_metadata_dialog(cx);
                    }),
                ))
                .child(primary_button(
                    "metadata-save",
                    "保存为新文件",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.start_metadata(cx);
                    }),
                )),
        )
        .child(
            div()
                .text_size(px(10.0))
                .text_color(text_faint())
                .whitespace_normal()
                .child(
                    "画面与音轨流复制到同目录的新 mkv，原文件不会被改动。\
                     标签留空即删除；不选封面就保留原封面，选了则把新图以附件写入。",
                ),
        )
        .into_any_element()
    }

    /// 封面那一行：当前选的图片 + 选择 / 移除。
    fn render_metadata_cover(&self, dialog: &MetadataDialog, cx: &mut Context<Self>) -> AnyElement {
        let (text, color) = match &dialog.cover {
            Some(path) => (file_name(path), text()),
            None => ("保持原样".to_string(), text_faint()),
        };
        div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .w(px(52.0))
                    .flex_none()
                    .text_size(px(12.0))
                    .text_color(text_muted())
                    .child("封面"),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(11.0))
                    .text_color(color)
                    .child(ui::ellipsis(text)),
            )
            .child(settings_button(
                "metadata-cover-pick",
                "选择图片…",
                true,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.pick_metadata_cover(window, cx);
                }),
            ))
            .when(dialog.cover.is_some(), |this| {
                this.child(settings_button(
                    "metadata-cover-clear",
                    "撤销",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.clear_metadata_cover(cx);
                    }),
                ))
            })
            .into_any_element()
    }

    fn render_metadata_progress(&self, progress: f32, cx: &mut Context<Self>) -> AnyElement {
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
                        "metadata-abort",
                        "取消",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.close_metadata_dialog(cx);
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

    fn render_metadata_done(&self, path: &Path, cx: &mut Context<Self>) -> AnyElement {
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
                        "metadata-reveal",
                        "定位",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.metadata_reveal_result(cx);
                        }),
                    ))
                    .child(settings_button(
                        "metadata-open",
                        "打开",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.metadata_open_result(cx);
                        }),
                    ))
                    .child(primary_button(
                        "metadata-finish",
                        "完成",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.close_metadata_dialog(cx);
                        }),
                    )),
            )
            .into_any_element()
    }
}

/// 一行标签：左边固定宽度的小标题，右边是组件库输入框。
fn tag_field(id: &'static str, label: &'static str, input: &Entity<InputState>) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .child(
            div()
                .w(px(52.0))
                .flex_none()
                .text_size(px(12.0))
                .text_color(text_muted())
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .child(Input::new(input).id(id).w_full()),
        )
        .into_any_element()
}
