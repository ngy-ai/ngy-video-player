//! 「收集素材到工程目录」：把素材复制到 `<工程目录>/<工程名>_assets/`，再把工程
//! 里的路径改过去存盘。之后整个文件夹可以整包拷到另一台机器上直接打开。
//!
//! 复制放在后台线程：素材动辄几个 G，同步拷会把界面冻住几十秒。计划（复制哪些、
//! 工程变成什么样）在主线程先算好，失败就什么都不改。

use std::path::{Path, PathBuf};

use super::*;

impl PlayerApp {
    /// 把当前工程的素材收进工程旁边的 `_assets` 目录。
    pub(super) fn collect_assets(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.project_path.clone() else {
            self.flash("先保存工程，再收集素材——收集要知道工程文件在哪");
            return;
        };
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let root = pack_dir(&path);
        let plan = match crate::timeline::pack::plan(project, &root) {
            Ok(plan) => plan,
            Err(error) => {
                self.flash(error.to_string());
                return;
            }
        };
        if plan.copies.is_empty() {
            self.flash("素材都已经在工程目录里了");
            return;
        }
        if let Err(error) = std::fs::create_dir_all(&root) {
            self.flash(format!("建不成目录「{}」：{error}", root.display()));
            return;
        }

        let count = plan.copies.len();
        let copies = plan.copies;
        let packed = plan.project;
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            for (from, to) in copies {
                if let Err(error) = std::fs::copy(&from, &to) {
                    let _ = tx.send(Err(format!("复制「{}」失败：{error}", from.display())));
                    return;
                }
            }
            let _ = tx.send(Ok(packed));
        });

        self.flash(format!("正在收集 {count} 个素材…"));
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = rx.await.ok();
            let _ = this.update(cx, |this, cx| {
                match result {
                    Some(Ok(project)) => {
                        // 路径全变了：先换工程，再按原路径存盘（`save` 会把素材路径
                        // 重算成相对工程文件的相对路径）。
                        this.project = Some(project);
                        this.project_saved = None;
                        this.save_project(None, cx, None);
                        this.flash(format!(
                            "已收集 {count} 个素材到「{}」",
                            root.file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        ));
                    }
                    Some(Err(message)) => {
                        // 复制到一半失败：工程没动过，删掉半成品目录只会更麻烦，
                        // 留给用户自己清理。
                        this.error = Some(Banner::new("收集素材失败", message));
                    }
                    None => {}
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// 素材目录：工程文件旁边的 `<工程名>_assets`。
fn pack_dir(project: &Path) -> PathBuf {
    let stem = project
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_string());
    match project.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(format!("{stem}_assets")),
        _ => PathBuf::from(format!("{stem}_assets")),
    }
}
