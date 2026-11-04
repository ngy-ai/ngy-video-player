//! 导出时间线到视频文件：计划校验、另存为对话框、后台任务与进度。
//!
//! 计划先生成再弹对话框：素材缺失、没有启用的视频轨这类问题，不该等用户选完
//! 路径才告诉他。

use std::path::{Path, PathBuf};

use super::project_file::{file_name, safe_file_stem};
use super::*;

impl PlayerApp {
    /// 标题条上的导出按钮。
    pub(super) fn export_sequence(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.export_task.is_some() {
            self.flash("已经有一个导出任务在跑，先取消或等它结束");
            return;
        }
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let plan = match crate::timeline::plan_sequence(project) {
            Ok(plan) => plan,
            Err(error) => {
                self.flash(format!("无法导出：{error}"));
                return;
            }
        };
        let total = project.sequence.duration();
        let default_name = format!("{}.mp4", safe_file_stem(&project.sequence.name, "导出"));
        // 优先落在工程旁边；没有工程路径就沿用上次打开文件的目录。
        let directory = self
            .project_path
            .as_ref()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .or_else(|| self.settings.last_dir.clone());

        if self.dialog_open {
            return;
        }
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
                    this.start_export(plan, path, total, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 取消正在跑的导出。任务线程会删掉半成品并回报 `Cancelled`。
    pub(super) fn cancel_export(&mut self, cx: &mut Context<Self>) {
        if let Some(task) = self.export_task.as_ref() {
            task.cancel();
        }
        self.export_task = None;
        self.export_progress = None;
        self.flash("正在取消导出…");
        cx.notify();
    }

    fn start_export(
        &mut self,
        plan: crate::timeline::ExportPlan,
        path: PathBuf,
        total: crate::timeline::Tick,
        cx: &mut Context<Self>,
    ) {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        match ExportTask::start(plan, path, total, tx) {
            Ok(task) => {
                self.export_task = Some(task);
                self.export_progress = Some(0.0);
                cx.spawn(async move |this, cx| {
                    let mut rx = rx;
                    while let Some(event) = rx.next().await {
                        let _ = this.update(cx, |this, cx| this.on_export_event(event, cx));
                    }
                })
                .detach();
            }
            Err(error) => self.flash(format!("启动 ffmpeg 失败：{error}")),
        }
    }

    fn on_export_event(&mut self, event: ExportEvent, cx: &mut Context<Self>) {
        match event {
            ExportEvent::Progress(fraction) => self.export_progress = Some(fraction),
            ExportEvent::Finished(path) => {
                self.export_task = None;
                self.export_progress = None;
                self.flash(format!("已导出「{}」", file_name(&path)));
            }
            ExportEvent::Failed(message) => {
                self.export_task = None;
                self.export_progress = None;
                self.error = Some(Banner::new("导出失败", message));
            }
            ExportEvent::Cancelled => {
                self.export_task = None;
                self.export_progress = None;
                self.flash("已取消导出");
            }
        }
        cx.notify();
    }
}
