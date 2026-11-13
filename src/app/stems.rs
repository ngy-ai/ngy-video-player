//! 导出音频分轨（stems）：选一个文件夹，把每条听得见的音频轨各写一个 wav。
//!
//! 这是 ADR 0011 里「不做 VST3/AU 宿主」的替代路径——要降噪、混响这类插件就把分轨
//! 导到外部 DAW，处理完再回导。计划本身在 `timeline::export::stems`，这里只管
//! 选目录、排队、进度与取消。
//!
//! 一次只跑一条 ffmpeg（与渲染缓存队列同一个取舍：同时开几条会互相抢 CPU，机器越忙
//! 总时间越长）。状态挂在 [`StemExport`] 上，worker 只把 `ExportEvent` 推回主线程，
//! 与 `render_cache_queue` 同一套代次机制：取消之后迟到的事件一律不算数。

use std::collections::VecDeque;
use std::sync::Arc;

use super::*;
use crate::timeline::{StemPlan, Tick, plan_stems};

/// 正在跑的分轨导出。`task` 留着是为了取消：丢掉 `ExportTask` 句柄不会杀掉子进程。
pub(super) struct StemExport {
    /// 还没开始的计划（当前那条已经出队）。
    pub pending: VecDeque<StemPlan>,
    pub done: usize,
    pub total: usize,
    /// 当前这轨的名字，横幅上要能看出在导哪一条。
    pub name: String,
    /// 当前这条的进度 0..=1；整批进度由 `done / total` 与它一起表达。
    pub progress: f32,
    /// 序列时长，`ExportTask` 用它换算进度。
    pub duration: Tick,
    pub directory: PathBuf,
    pub task: Option<Arc<ExportTask>>,
}

impl StemExport {
    /// 横幅上的一行字。进度按当前这条算，序号是 1 起（「第几条」比「已完成几条」好读）。
    pub fn label(&self) -> String {
        let percent = self.progress.clamp(0.0, 1.0) * 100.0;
        format!(
            "分轨 {}/{} · {:.0}% · {}",
            (self.done + 1).min(self.total.max(1)),
            self.total,
            percent.max(0.0),
            self.name
        )
    }
}

impl PlayerApp {
    /// 「文件 > 导出音频分轨…」：先选目录，再逐条导出。
    pub(super) fn export_audio_stems(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.is_none() {
            self.flash("先打开工程，再导出分轨");
            cx.notify();
            return;
        }
        // 一次只跑一队：叠着跑会把 CPU 分薄，也会让进度分不清是哪一队。
        if self.stem_export.is_some() {
            self.flash("分轨导出还在跑，等它结束或点横幅上的取消");
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

    /// 生成计划并排队。计划失败（没有可导出的轨）只弹提示，不留半截文件。
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
        let duration = project.sequence().duration();
        let count = stems.len();
        let label = file_name(&directory).to_string();
        self.remember_dir(&directory);
        // 换一队就换一代：上一队取消 / 收尾后还在路上的事件不再改新队的状态。
        self.stem_export_generation = self.stem_export_generation.wrapping_add(1);
        self.stem_export = Some(StemExport {
            pending: stems.into(),
            done: 0,
            total: count,
            name: String::new(),
            progress: 0.0,
            duration,
            directory,
            task: None,
        });
        self.flash(format!("正在导出 {count} 条分轨到「{label}」…"));
        self.start_next_stem(cx);
        cx.notify();
    }

    /// 起下一条；队列空了就收尾。事件全部经 `cx.spawn` 回到主线程改状态。
    fn start_next_stem(&mut self, cx: &mut Context<Self>) {
        let generation = self.stem_export_generation;
        let Some(stem) = self
            .stem_export
            .as_mut()
            .and_then(|state| state.pending.pop_front())
        else {
            self.finish_stem_export(cx);
            return;
        };
        let name = stem.name.clone();
        if let Some(state) = self.stem_export.as_mut() {
            state.name = name.clone();
            state.progress = 0.0;
        }
        let duration = self
            .stem_export
            .as_ref()
            .map(|state| state.duration)
            .unwrap_or(Tick::ZERO);

        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let task = match ExportTask::start(stem.plan, stem.path.clone(), duration, tx) {
            Ok(task) => task,
            Err(error) => {
                self.abort_stem_export(format!("「{name}」起不来 ffmpeg：{error}"), cx);
                return;
            }
        };
        if let Some(state) = self.stem_export.as_mut() {
            state.task = Some(Arc::new(task));
        }

        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let alive = this.update(cx, |this, cx| {
                    if this.stem_export_generation != generation {
                        return;
                    }
                    match event {
                        ExportEvent::Progress(progress) => {
                            if let Some(state) = this.stem_export.as_mut() {
                                state.progress = progress;
                            }
                        }
                        ExportEvent::Finished(_) => this.finish_stem(None, cx),
                        ExportEvent::Failed(message) => this.finish_stem(Some(message), cx),
                        ExportEvent::Cancelled => {
                            this.finish_stem(Some("被取消".to_string()), cx);
                        }
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// 一条跑完（或失败）。句柄在这一刻才丢，提前丢等于把子进程掐了。
    fn finish_stem(&mut self, failure: Option<String>, cx: &mut Context<Self>) {
        let (done, total, name) = match self.stem_export.as_mut() {
            Some(state) => {
                state.task = None;
                (state.done, state.total, state.name.clone())
            }
            None => return,
        };
        if let Some(message) = failure {
            self.abort_stem_export(
                if done > 0 {
                    format!("分轨导出中断（已完成 {done}/{total}）：「{name}」{message}")
                } else {
                    format!("分轨导出失败：「{name}」{message}")
                },
                cx,
            );
            return;
        }
        if let Some(state) = self.stem_export.as_mut() {
            state.done += 1;
        }
        self.start_next_stem(cx);
    }

    /// 整队跑完。
    fn finish_stem_export(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.stem_export.take() else {
            return;
        };
        let label = file_name(&state.directory).to_string();
        self.flash(format!(
            "已导出 {}/{} 条分轨到「{label}」",
            state.done, state.total
        ));
        cx.notify();
    }

    /// 失败 / 起不来 ffmpeg：整队收掉，别让后面的轨写出半套文件。
    fn abort_stem_export(&mut self, message: String, cx: &mut Context<Self>) {
        self.stem_export = None;
        self.stem_export_generation = self.stem_export_generation.wrapping_add(1);
        self.flash(message);
        cx.notify();
    }

    /// 横幅上的「取消」：杀掉当前 ffmpeg（半成品由 `ExportTask` 自己删），清掉剩下的队列。
    pub(super) fn cancel_stem_export(&mut self, cx: &mut Context<Self>) {
        let Some(state) = self.stem_export.take() else {
            return;
        };
        self.stem_export_generation = self.stem_export_generation.wrapping_add(1);
        if let Some(task) = state.task.as_ref() {
            task.cancel();
        }
        self.flash(format!(
            "已取消分轨导出（已完成 {}/{}）",
            state.done, state.total
        ));
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(done: usize, total: usize, progress: f32) -> StemExport {
        StemExport {
            pending: VecDeque::new(),
            done,
            total,
            name: "音频 1".to_string(),
            progress,
            duration: Tick::ZERO,
            directory: PathBuf::new(),
            task: None,
        }
    }

    #[test]
    fn 横幅文字按一起的序号且进度夹在区间内() {
        assert_eq!(state(0, 3, 0.25).label(), "分轨 1/3 · 25% · 音频 1");
        assert_eq!(state(2, 3, 1.0).label(), "分轨 3/3 · 100% · 音频 1");
        assert_eq!(state(0, 3, -2.0).label(), "分轨 1/3 · 0% · 音频 1");
        assert_eq!(state(0, 3, 4.0).label(), "分轨 1/3 · 100% · 音频 1");
        // 全跑完的瞬间 done == total，序号不能越到 total+1。
        assert_eq!(state(3, 3, 1.0).label(), "分轨 3/3 · 100% · 音频 1");
    }
}
