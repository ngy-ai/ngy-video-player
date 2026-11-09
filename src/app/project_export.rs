//! 导出时间线到视频文件：另存为对话框、后台任务与进度。
//!
//! 参数与预设先由 [`PlayerApp::open_export_dialog`] 收齐，这里从「选文件名」开始：
//! 计划先生成再弹对话框：素材缺失、没有启用的视频轨这类问题，不该等用户选完
//! 路径才告诉他。

use std::path::{Path, PathBuf};

use super::project_file::{file_name, safe_file_stem};
use super::*;
use crate::timeline::{ExportSettings, Tick};

impl PlayerApp {
    /// 选文件名并把这次导出排进队列。
    ///
    /// `settings` 是面板上确认过的编码参数；`preset` 是它与哪份预设完全一致，
    /// 队列项与进度提示会把它显示出来（改了参数就是 `None`，即「自定义」）。
    pub(super) fn pick_export_path(
        &mut self,
        settings: ExportSettings,
        preset: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 原生选择框一次只开一个：连按 O / 连点导出不会叠出多个。
        if self.dialog_open {
            self.flash("已经有一个文件选择框开着，先把它处理掉");
            cx.notify();
            return;
        }
        let Some(project) = self.project.as_ref() else {
            return;
        };
        // 有入出点就只导工作区：这是「先粗剪一遍，再只导那一段」的入口。
        let range = project.ui.range();
        let plan = match crate::timeline::plan_sequence_with(project, range, &settings) {
            Ok(plan) => plan,
            Err(error) => {
                self.flash(format!("无法导出：{error}"));
                return;
            }
        };
        let total = match range {
            Some((start, end)) => end - start,
            None => project.sequence.duration(),
        };
        let stem = safe_file_stem(&project.sequence.name, "导出");
        let default_name = if range.is_some() {
            format!("{stem} 工作区.mp4")
        } else {
            format!("{stem}.mp4")
        };
        // 优先落在工程旁边；没有工程路径就沿用上次打开文件的目录。
        let directory = self
            .project_path
            .as_ref()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .or_else(|| self.settings.last_dir.clone());

        self.dialog_open = true;

        let mut picker = rfd::FileDialog::new()
            .add_filter("MP4 视频", &["mp4"])
            .set_file_name(&default_name)
            .set_parent(window);
        if let Some(directory) = directory {
            picker = picker.set_directory(directory);
        }

        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(picker.save_file());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if let Some(path) = picked {
                    if let Some((start, end)) = range {
                        this.flash(format!(
                            "只导出工作区 {:.2}–{:.2} 秒",
                            start.secs(),
                            end.secs()
                        ));
                    }
                    this.enqueue_export(plan, path, total, preset, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 取消正在跑的导出。任务线程会删掉半成品并回报 `Cancelled`。
    ///
    /// 队列里排着的也一并清掉：× 只出现在进度旁边，用户按它的意思是「别导了」，
    /// 悄悄接着跑后面几条会像是取消没生效。
    pub(super) fn cancel_export(&mut self, cx: &mut Context<Self>) {
        if let Some(task) = self.export_task.as_ref() {
            task.cancel();
        }
        let queued = self.export_queue.len();
        self.export_queue.clear();
        self.export_task = None;
        self.export_progress = None;
        self.export_preset = None;
        self.flash(if queued == 0 {
            "正在取消导出…".to_string()
        } else {
            format!("正在取消导出，队列里 {queued} 条也一并清掉")
        });
        cx.notify();
    }

    pub(super) fn start_export(
        &mut self,
        plan: crate::timeline::ExportPlan,
        path: PathBuf,
        total: Tick,
        preset: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        match ExportTask::start(plan, path, total, tx) {
            Ok(task) => {
                self.export_task = Some(task);
                self.export_progress = Some(0.0);
                self.export_preset = preset;
                cx.spawn(async move |this, cx| {
                    let mut rx = rx;
                    while let Some(event) = rx.next().await {
                        let _ = this.update(cx, |this, cx| this.on_export_event(event, cx));
                    }
                })
                .detach();
            }
            Err(error) => {
                // 连 ffmpeg 都起不来，后面几条也一定一样：整队一起停，别让用户
                // 盯着一个永远不动的「队列 3」。
                self.export_queue.clear();
                self.flash(format!("启动 ffmpeg 失败：{error}"));
            }
        }
    }

    fn on_export_event(&mut self, event: ExportEvent, cx: &mut Context<Self>) {
        match event {
            ExportEvent::Progress(fraction) => self.export_progress = Some(fraction),
            ExportEvent::Finished(path) => {
                self.export_task = None;
                self.export_progress = None;
                let name = file_name(&path);
                match self.export_preset.take() {
                    Some(preset) => self.flash(format!("已导出「{name}」（预设「{preset}」）")),
                    None => self.flash(format!("已导出「{name}」")),
                }
                self.start_next_export(cx);
            }
            ExportEvent::Failed(message) => {
                // 一条失败不阻塞后面：横幅留着说明原因，队列继续。
                self.export_task = None;
                self.export_progress = None;
                self.export_preset = None;
                self.error = Some(Banner::new("导出失败", message));
                self.start_next_export(cx);
            }
            ExportEvent::Cancelled => {
                self.export_task = None;
                self.export_progress = None;
                self.export_preset = None;
                self.flash("已取消导出");
            }
        }
        cx.notify();
    }
}
