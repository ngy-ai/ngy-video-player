//! 工程的打开 / 新建 / 关闭与未保存确认。

use super::*;

impl PlayerApp {
    /// 退出工程模式。未保存改动由调用方先确认（见 `request_project_action`）。
    pub(in crate::app) fn close_project(&mut self, cx: &mut Context<Self>) {
        // 预览播放属于工程：工程关掉了，时钟和 mpv 都不该继续跑。
        self.stop_timeline_playback(cx);
        self.stop_timeline_audio();
        self.leave_composite();
        // 素材库、导入进度与试听都是上一个工程的：先清掉，再拆工程。
        self.reset_library();
        if self.project.take().is_some() {
            // 内容已经存过或被用户主动丢弃，`.autosave` 再留着只会下次白问一句。
            self.clear_autosave();
            self.project_path = None;
            self.project_saved = None;
            self.history = None;
            self.show_history = false;
            self.delete_track_confirm = None;
            // 序列规格对话框作用在刚被关掉的工程上，留着就成了悬空入口。
            self.sequence_dialog = None;
            self.pending_sequence_dialog = None;
            self.timeline_dragging = false;
            self.timeline_menu = None;
            self.timeline_preview = None;
            self.timeline_loaded_at = None;
            self.flash("已关闭工程");
        }
        self.poke();
        cx.notify();
    }

    /// 新建一个空工程：规格来自「新建工程」对话框，自带一条视频轨与一条音频轨。
    pub(in crate::app) fn new_project(
        &mut self,
        spec: crate::timeline::SequenceSpec,
        cx: &mut Context<Self>,
    ) {
        // 换工程 = 换播放源：上一个工程的预览时钟和已加载的素材都作废。
        self.stop_timeline_playback(cx);
        self.stop_timeline_audio();
        self.leave_composite();
        self.reset_library();
        // 普通播放里的上一部片子不属于这个空工程：连画面带播放一起停掉，
        // 否则它的画面会一直挂在预览区，还会随着 mpv 继续解码不停刷新。
        self.stop_and_clear_frame(cx);
        self.timeline_preview = None;
        self.timeline_loaded_at = None;
        // 上一个工程已经被保存或丢弃，它的 `.autosave` 不该跟着新工程活下来。
        self.clear_autosave();
        let project = Project::new_with(spec);
        // 空工程还没落盘，但也没丢任何东西：把当前状态当基准，关掉时不必再问。
        self.project_saved = Some(project.clone());
        self.project_path = None;
        self.history = Some(UndoHistory::new(project.sequence().clone()));
        self.show_history = false;
        self.delete_track_confirm = None;
        self.project = Some(project);
        self.timeline = ViewState::new();
        self.timeline_dragging = false;
        self.flash(format!("已新建工程（{}）", spec.label()));
        self.poke();
        cx.notify();
    }

    /// 选一个 `.ngyproj` 打开。对话框阻塞在后台线程里，结果走 oneshot 回来。
    pub(in crate::app) fn pick_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let mut picker = rfd::FileDialog::new()
            .add_filter("工程文件", formats::PROJECT)
            .add_filter("所有文件", &["*"])
            .set_parent(window);
        if let Some(dir) = &self.settings.last_dir {
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
                if let Some(path) = picked {
                    this.remember_dir(&path);
                    // 当前工程可能还有没保存的东西，先过一遍确认。
                    this.request_project_action(AfterSaveConfirm::OpenProject(path), cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 真正读一份 `.ngyproj` 进内存。
    ///
    /// `recovery` 是用户同意恢复时读的那份自动保存。入口在
    /// [`PlayerApp::open_project_with_recovery`]：那边先问一句，这里只管装载。
    pub(in crate::app) fn load_project(
        &mut self,
        path: PathBuf,
        recovery: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let restored = recovery.is_some();
        let source = recovery.unwrap_or_else(|| path.clone());
        // 换工程 = 换播放源：上一个工程的预览时钟和已加载的素材都作废。
        self.stop_timeline_playback(cx);
        self.stop_timeline_audio();
        self.leave_composite();
        self.reset_library();
        self.timeline_preview = None;
        self.timeline_loaded_at = None;
        // 上一个工程的菜单指向的片段在这个工程里不存在。
        self.timeline_menu = None;
        match Project::open(&source) {
            Ok(project) => {
                let duration = project.sequence().duration();
                let playhead = project.ui.playhead.secs();
                self.timeline = ViewState::new();
                match project.ui.zoom {
                    // 老工程没存过视图状态，按播放头居中打开。
                    Some(zoom) => {
                        self.timeline.set_zoom(
                            zoom,
                            ViewState::max_zoom(project.sequence().fps),
                            duration,
                        );
                        self.timeline.scroll_to(project.ui.scroll, duration);
                    }
                    None => self.timeline.center_on(playhead, duration),
                }
                self.timeline_dragging = false;
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "工程".to_string());
                // 从自动保存恢复的内容还没落进工程文件：基准取盘上那份，否则
                // 「未保存」标记会消失，用户以为已经存过了。盘上那份读不出来时
                // 基准留空，同样按「有未保存改动」对待。
                self.project_saved = if restored {
                    Project::open(&path).ok()
                } else {
                    Some(project.clone())
                };
                self.remember_project(&path);
                self.project_path = Some(path);
                // 认不出来的效果不阻止打开，但要说一句：否则用户会以为效果丢了。
                let unknown = crate::timeline::unknown_effects(project.sequence());
                self.history = Some(UndoHistory::new(project.sequence().clone()));
                self.show_history = false;
                self.delete_track_confirm = None;
                self.project = Some(project);
                if restored {
                    // 恢复出来的改动也怕再崩一次：重新排上自动保存。
                    self.note_edit();
                    self.flash("已从自动保存恢复，记得保存工程");
                } else {
                    self.autosave.mark_saved();
                    let mut message = format!("已打开工程「{name}」");
                    if unknown > 0 {
                        message.push_str(&format!("；{unknown} 个不认识的效果已跳过"));
                    }
                    self.flash(message);
                }
                // 打开就出一帧：进来先看到黑屏，会让人以为工程打不开。
                self.timeline_preview = None;
                self.preview_at_playhead(cx);
            }
            Err(error) => self.flash(format!("打开工程失败：{error}")),
        }
        self.poke();
        cx.notify();
    }
}
