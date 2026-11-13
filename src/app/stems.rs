//! 导出音频分轨（stems）：选一个文件夹，把每条听得见的音频轨各写一个 wav。
//!
//! 这是 ADR 0011 里「不做 VST3/AU 宿主」的替代路径——要降噪、混响这类插件就把分轨
//! 导到外部 DAW，处理完再回导。计划本身在 `timeline::export::stems`，这里只管
//! 选目录、排队与提示。
//!
//! 一次只跑一条 ffmpeg（与渲染缓存队列同一个取舍：同时开几条会互相抢 CPU，机器越忙
//! 总时间越长）。没有进度条与取消按钮：一条分轨通常几秒到几十秒，弹一条横幅说
//! 「正在导出 2/3」就够；真要做取消，按 `render_cache_queue` 的样子加一个队列状态。

use super::*;
use crate::timeline::plan_stems;

impl PlayerApp {
    /// 「文件 > 导出音频分轨…」：先选目录，再逐条导出。
    pub(super) fn export_audio_stems(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.is_none() {
            self.flash("先打开工程，再导出分轨");
            cx.notify();
            return;
        }
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let mut picker = rfd::FileDialog::new().set_title("选一个文件夹放分轨");
        if let Some(directory) = self.settings.last_dir.clone() {
            picker = picker.set_directory(directory);
        }
        picker = picker.set_parent(window);

        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(picker.pick_folder());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if let Some(directory) = picked {
                    this.start_stem_export(directory, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 生成计划并逐条跑。计划失败（没有可导出的轨）只弹提示，不留半截文件。
    fn start_stem_export(&mut self, directory: PathBuf, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let stems = match plan_stems(project, &directory) {
            Ok(stems) => stems,
            Err(error) => {
                self.flash(error.to_string());
                cx.notify();
                return;
            }
        };
        let total = project.sequence().duration();
        self.remember_dir(&directory);
        let count = stems.len();
        self.flash(format!(
            "正在导出 {count} 条分轨到「{}」…",
            file_name(&directory)
        ));

        cx.spawn(async move |this, cx| {
            let mut done = 0;
            let mut failure = None;
            for stem in stems {
                let name = stem.name.clone();
                let (tx, mut rx) = futures::channel::mpsc::unbounded();
                let task = match ExportTask::start(stem.plan, stem.path.clone(), total, tx) {
                    Ok(task) => task,
                    Err(error) => {
                        failure = Some(format!("「{name}」起不来 ffmpeg：{error}"));
                        break;
                    }
                };
                let mut outcome = None;
                while let Some(event) = rx.next().await {
                    match event {
                        ExportEvent::Progress(_) => {}
                        ExportEvent::Finished(_) => break,
                        ExportEvent::Failed(message) => {
                            outcome = Some(message);
                            break;
                        }
                        ExportEvent::Cancelled => {
                            outcome = Some("被取消".to_string());
                            break;
                        }
                    }
                }
                // 句柄留着直到这一条真的结束：提前丢掉等于把子进程掐了。
                drop(task);
                match outcome {
                    Some(message) => {
                        failure = Some(format!("「{name}」失败：{message}"));
                        break;
                    }
                    None => {
                        done += 1;
                        let _ = this.update(cx, |this, cx| {
                            this.flash(format!("分轨 {done}/{count} 已导出"));
                            cx.notify();
                        });
                    }
                }
            }

            let _ = this.update(cx, |this, cx| {
                match failure {
                    Some(message) => {
                        this.flash(if done > 0 {
                            format!("分轨导出中断（已完成 {done}/{count}）：{message}")
                        } else {
                            format!("分轨导出失败：{message}")
                        });
                    }
                    None => this.flash(format!(
                        "已导出 {done} 条分轨到「{}」",
                        file_name(&directory)
                    )),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
