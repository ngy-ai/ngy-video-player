//! 自动保存的界面侧接入：什么时候算改过、到点怎么写、崩溃之后怎么恢复。
//!
//! 纯计时在 `crate::timeline::autosave` 里（单测里快进 30 秒不用真睡），这里只补
//! 它管不到的三件事：
//!
//! 1. **算作一次编辑**：所有改序列的路径最后都要经过 `remember_history`
//!    或 `apply_history`，计时就挂在那两个地方，不必逐条去记；
//! 2. **落盘**：后台每秒心跳一次，到点就在主线程写一份 `.autosave`（一份工程只有
//!    几十 KB，比把 `Project` 搬到线程里更省事，也不会和正常保存抢文件）；
//! 3. **问用户**：打开工程时发现更晚的 `.autosave`，先问要不要恢复。
//!
//! `.autosave` 挂在工程文件旁边，所以**还没落过盘的工程没有自动保存**——没有地方放
//! 那份文件。第一次「保存」之后就有了。

use super::*;

impl PlayerApp {
    /// 记一次编辑：从现在重新开始数 30 秒。
    pub(super) fn note_edit(&mut self) {
        self.autosave.mark_dirty(self.started.elapsed());
    }

    /// 后台心跳。判断在 `tick_autosave` 里，这里只负责长期挂着。
    ///
    /// 秒级粒度够了：要的是「安静 30 秒」，不是精确到毫秒的定时器。
    pub(super) fn start_autosave_watch(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let alive = this.update(cx, |this, cx| {
                    if this.tick_autosave() {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// 到点就写一份 `.autosave`。返回是否真的写了。
    fn tick_autosave(&mut self) -> bool {
        let now = self.started.elapsed();
        // 没有工程，或者跟盘上那份一致：没什么可存。顺手清掉计时，
        // 免得每次心跳都重新比一遍内容。
        if self.project.is_none() || !self.project_modified() {
            self.autosave.mark_saved();
            return false;
        }
        // 还没落过盘：没有落点放 `.autosave`，等用户第一次保存。
        let Some(path) = self.project_path.clone() else {
            self.autosave.mark_saved();
            return false;
        };
        if !self.autosave.should_write(now) {
            return false;
        }
        // 视图状态跟着一起存：恢复之后看得还是崩溃前那个位置。
        self.stamp_view_state();
        let written = match self.project.as_ref() {
            Some(project) => autosave::write(project, &path),
            None => return false,
        };
        match written {
            Ok(file) => {
                log_info!("已自动保存工程: {}", file.display());
                self.flash("已自动保存一份副本");
                true
            }
            Err(error) => {
                // 磁盘临时满、被别的程序锁住都可能只是这一下：重新排上再来。
                log_warn!("自动保存失败: {error}");
                self.autosave.mark_dirty(now);
                false
            }
        }
    }

    /// 打开工程前先问一句：上次是不是崩在这份工程上。
    ///
    /// 只有比工程文件**更晚**的 `.autosave` 才算数（判断在 `autosave::pending`），
    /// 正常保存与正常退出都会把它删掉，所以这里很少真的弹出。
    pub(super) fn open_project_with_recovery(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(autosave_file) = autosave::pending(&path) else {
            self.load_project(path, None, cx);
            return;
        };
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let dialog = rfd::MessageDialog::new()
            .set_title("发现自动保存")
            .set_description(format!(
                "「{name}」旁边有一份比工程文件更新的自动保存，上次可能没有正常退出。\n\n\
                 要恢复它吗？选「否」会把它改名成 .autosave.rejected 留着，不再提示。"
            ))
            .set_buttons(rfd::MessageButtons::YesNo);
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(dialog.show());
        });

        cx.spawn(async move |this, cx| {
            // 拿不到结果就按「不恢复」走：盘上那份工程永远是更保守的选择。
            let choice = rx.await.ok();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if matches!(choice, Some(rfd::MessageDialogResult::Yes)) {
                    this.load_project(path, Some(autosave_file), cx);
                } else {
                    if let Err(error) = autosave::dismiss(&path) {
                        log_warn!("改名自动保存失败: {error}");
                    }
                    this.load_project(path, None, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }
}
