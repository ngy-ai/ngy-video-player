//! 工程文件的写盘那一半：保存、另存为、未保存确认。
//!
//! 工程入口（新建 / 打开 / 关闭）在 `timeline_view.rs`，这里只补上写盘与拦截：
//! 任何会丢掉当前工程的动作用 [`PlayerApp::request_project_action`] 走一遍，
//! 有未保存改动就先让用户选「保存 / 不保存 / 取消」。
//!
//! 确认框用系统对话框而不是自绘模态：它要挡在一个**还没发生**的动作前面，
//! 自绘模态得再往 `render()` 的互斥分支里塞一套状态，收益不成比例。

use std::path::Path;

use super::*;
use crate::timeline::PROJECT_EXTENSION;

impl PlayerApp {
    /// 有没有未保存改动。
    pub(super) fn project_modified(&self) -> bool {
        match (&self.project, &self.project_saved) {
            (Some(project), Some(saved)) => !project.same_content(saved),
            // 有工程却没有基准，说明它压根没落过盘。
            (Some(_), None) => true,
            _ => false,
        }
    }

    /// 保存到当前路径；从没存过就转成「另存为」。
    ///
    /// 返回 `false` 表示程序已经在退出的路上，调用方别再碰 `cx`。
    pub(super) fn save_project(
        &mut self,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
        then: Option<AfterSaveConfirm>,
    ) -> bool {
        let Some(path) = self.project_path.clone() else {
            self.save_project_as(window, cx, then);
            return true;
        };
        if self.write_project(&path) {
            let alive = self.continue_after(then, cx);
            if alive {
                cx.notify();
            }
            return alive;
        }
        // 存盘失败：不再继续后面的动作，否则等于替用户选了「不保存」。
        cx.notify();
        true
    }

    /// 换一个路径保存。对话框在后台线程里阻塞，结果走 oneshot 回来。
    pub(super) fn save_project_as(
        &mut self,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
        then: Option<AfterSaveConfirm>,
    ) {
        if self.dialog_open {
            return;
        }
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let default_name = default_project_file_name(&project.sequence.name);
        // 优先落在工程旁边；新建的工程还没有位置，就沿用上次打开文件的目录。
        let directory = self
            .project_path
            .as_ref()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .or_else(|| self.settings.last_dir.clone());
        self.dialog_open = true;

        let mut picker = rfd::FileDialog::new()
            .add_filter("工程文件", formats::PROJECT)
            .set_file_name(&default_name);
        if let Some(directory) = directory {
            picker = picker.set_directory(directory);
        }
        if let Some(window) = window {
            picker = picker.set_parent(window);
        }

        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(picker.save_file());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                let alive = match picked {
                    Some(path) => {
                        let path = with_project_extension(path);
                        this.remember_dir(&path);
                        if this.write_project(&path) {
                            this.continue_after(then, cx)
                        } else {
                            true
                        }
                    }
                    // 用户取消了对话框：后面的动作也一并取消。
                    None => true,
                };
                if alive {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 做一件会丢掉当前工程的事之前先问一句。
    ///
    /// 没有工程、或者没有未保存改动时直接执行，不打扰。
    pub(super) fn request_project_action(
        &mut self,
        action: AfterSaveConfirm,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() || !self.project_modified() {
            self.run_project_action(action, cx);
            return;
        }
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let mut dialog = rfd::MessageDialog::new()
            .set_title("工程还没有保存")
            .set_description("当前工程有未保存的改动。要保存之后再继续吗？")
            .set_buttons(rfd::MessageButtons::YesNoCancel);
        if let Some(window) = window {
            dialog = dialog.set_parent(window);
        }

        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(dialog.show());
        });

        cx.spawn(async move |this, cx| {
            let choice = rx.await.unwrap_or(rfd::MessageDialogResult::Cancel);
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                let alive = match choice {
                    rfd::MessageDialogResult::Yes => {
                        this.save_project(None, cx, Some(action));
                        true
                    }
                    rfd::MessageDialogResult::No => this.run_project_action(action, cx),
                    _ => true,
                };
                if alive {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 真正执行一个「换工程」的动作。返回 `false` 表示程序正在退出。
    fn run_project_action(&mut self, action: AfterSaveConfirm, cx: &mut Context<Self>) -> bool {
        match action {
            AfterSaveConfirm::NewProject => {
                self.new_project(cx);
                true
            }
            AfterSaveConfirm::CloseProject => {
                self.close_project(cx);
                true
            }
            AfterSaveConfirm::OpenProject(path) => {
                self.open_project_path(path, cx);
                true
            }
            AfterSaveConfirm::Quit => {
                cx.quit();
                false
            }
        }
    }

    fn continue_after(&mut self, then: Option<AfterSaveConfirm>, cx: &mut Context<Self>) -> bool {
        match then {
            Some(action) => self.run_project_action(action, cx),
            None => true,
        }
    }

    /// 落盘并刷新基准。返回是否成功。
    fn write_project(&mut self, path: &Path) -> bool {
        // 视图状态跟着工程走：下次打开还停在最后一次看的位置。
        let zoom = self.timeline.zoom();
        let scroll = self.timeline.scroll();
        let outcome = match self.project.as_mut() {
            Some(project) => {
                project.ui.zoom = Some(zoom);
                project.ui.scroll = scroll;
                project.save(path)
            }
            None => return false,
        };
        match outcome {
            Ok(()) => {
                self.project_path = Some(path.to_path_buf());
                // `save` 会把素材路径改写成相对形式，基准必须在这之后取。
                self.project_saved = self.project.clone();
                self.flash(format!("已保存工程「{}」", file_name(path)));
                true
            }
            Err(error) => {
                log_warn!("保存工程失败: {error}");
                self.error = Some(Banner::new(
                    "保存工程失败",
                    format!("{error}\n\n工程还在内存里，可以换个位置再存一次。"),
                ));
                false
            }
        }
    }
}

/// 把名字里的文件名非法字符换掉，并清掉结尾的空格与点。
///
/// Windows 不允许文件名以空格或点结尾；工程文件与导出文件共用这一套清洗。
pub(super) fn safe_file_stem(name: &str, fallback: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| if r#"\:*?"<>|/"#.contains(ch) { '_' } else { ch })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches('.');
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned.to_string()
    }
}

/// 另存为对话框里的默认文件名：序列名去掉文件名非法字符，再加上扩展名。
fn default_project_file_name(sequence_name: &str) -> String {
    format!(
        "{}.{PROJECT_EXTENSION}",
        safe_file_stem(sequence_name, "工程")
    )
}

/// 用户在对话框里删掉了扩展名时补回去，别存出一个打不开的工程。
fn with_project_extension(path: PathBuf) -> PathBuf {
    if formats::is_project(&path) {
        return path;
    }
    let mut name = path.into_os_string();
    name.push(".");
    name.push(PROJECT_EXTENSION);
    PathBuf::from(name)
}

pub(super) fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 另存为的默认文件名清掉非法字符() {
        assert_eq!(default_project_file_name("序列 1"), "序列 1.ngyproj");
        assert_eq!(default_project_file_name("a/b:c*d?"), "a_b_c_d_.ngyproj");
        assert_eq!(default_project_file_name("   "), "工程.ngyproj");
        assert_eq!(default_project_file_name("尾巴...."), "尾巴.ngyproj");
    }

    #[test]
    fn 保存路径缺扩展名时补上() {
        assert_eq!(
            with_project_extension(PathBuf::from("demo")),
            PathBuf::from("demo.ngyproj")
        );
        assert_eq!(
            with_project_extension(PathBuf::from("demo.ngyproj")),
            PathBuf::from("demo.ngyproj")
        );
        // 别的扩展名不是「已经带了」，要接在后面而不是替换。
        assert_eq!(
            with_project_extension(PathBuf::from("demo.txt")),
            PathBuf::from("demo.txt.ngyproj")
        );
    }
}
