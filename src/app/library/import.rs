//! 三个导入入口（选文件 / 选文件夹 / 系统拖入都汇到 `import_assets`）与后台探测。

use super::super::*;
use super::panel::LibraryImport;
use crate::timeline::{AssetState, MediaInfo};
use std::path::PathBuf;

impl PlayerApp {
    /// 文件选择框：选一批文件进素材库。
    pub(crate) fn pick_assets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.is_none() || self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let mut picker = rfd::FileDialog::new()
            .add_filter("视频", formats::VIDEO)
            .add_filter("音频", formats::AUDIO)
            .set_title("导入素材");
        if let Some(directory) = self.settings.last_dir.clone() {
            picker = picker.set_directory(directory);
        }
        picker = picker.set_parent(window);

        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(picker.pick_files());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if let Some(paths) = picked {
                    this.import_assets(paths, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 选一个文件夹，递归导入里面所有能剪的媒体。
    pub(crate) fn pick_asset_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.is_none() || self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let mut picker = rfd::FileDialog::new().set_title("导入文件夹到素材库");
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
                if let Some(folder) = picked {
                    this.import_assets(vec![folder], cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 把文件 / 文件夹收进素材库。已登记的路径不会重复登记。
    ///
    /// 登记是同步的（只写素材表），探测放后台：界面要立刻能看到新行，
    /// 而每个文件的 `ffmpeg -i` 要几十毫秒，几百个就是十几秒。
    pub(crate) fn import_assets(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.project.is_none() {
            return;
        }
        if let Some(first) = paths.first() {
            self.remember_dir(first);
        }

        // 文件夹先展开成文件。扫描本身只读目录项，几百个文件也就几十毫秒。
        let mut files = Vec::new();
        for path in paths {
            if path.is_dir() {
                match crate::convert::scan_folder(&path) {
                    Ok(found) => files.extend(found),
                    Err(error) => {
                        log_warn!("扫描素材文件夹失败：{error:#}");
                    }
                }
            } else if formats::is_media(&path) {
                files.push(path);
            }
        }

        let mut to_probe: Vec<PathBuf> = Vec::new();
        let mut known = 0usize;
        {
            let Some(project) = self.project.as_mut() else {
                return;
            };
            for path in files {
                let id = project.add_asset(&path);
                let cached = project.asset(id).is_some_and(|asset| {
                    asset.media.is_some() && asset.state() == AssetState::Ready
                });
                if cached {
                    known += 1;
                } else if !to_probe.contains(&path) {
                    to_probe.push(path);
                }
            }
        }

        if to_probe.is_empty() {
            self.refresh_library_states();
            if known > 0 {
                self.flash(format!("这 {known} 个素材已经在素材库里了"));
            } else {
                self.flash("没有可导入的媒体文件");
            }
            cx.notify();
            return;
        }

        self.library_generation = self.library_generation.wrapping_add(1);
        let generation = self.library_generation;
        let total = to_probe.len();
        self.library_import = Some(LibraryImport {
            total,
            done: 0,
            failed: 0,
        });
        // 新加进来的素材状态未知，先把缓存刷一遍：缺文件的照样能看见。
        self.refresh_library_states();

        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        std::thread::Builder::new()
            .name("library-probe".into())
            .spawn(move || {
                for path in to_probe {
                    let result = crate::media::probe(&path);
                    if tx.unbounded_send((path, result)).is_err() {
                        break;
                    }
                }
            })
            .map_err(|error| log_warn!("素材探测线程启动失败: {error}"))
            .ok();

        cx.spawn(async move |this, cx| {
            while let Some((path, result)) = rx.next().await {
                let alive = this.update(cx, |this, cx| {
                    this.on_probe_result(generation, path, result, cx);
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// 后台探测回来一条。代数对不上（换过工程）就直接丢掉。
    fn on_probe_result(
        &mut self,
        generation: u64,
        path: PathBuf,
        result: Result<MediaInfo>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.library_generation {
            return;
        }
        let Some(project) = self.project.as_mut() else {
            return;
        };
        match result {
            Ok(info) => {
                if let Err(error) = crate::media::register_probed(project, &path, info) {
                    log_warn!("登记素材「{}」失败：{error:#}", path.display());
                    if let Some(import) = self.library_import.as_mut() {
                        import.failed += 1;
                    }
                }
            }
            Err(error) => {
                log_warn!("探测素材「{}」失败：{error:#}", path.display());
                if let Some(import) = self.library_import.as_mut() {
                    import.failed += 1;
                }
            }
        }
        let finished = {
            let Some(import) = self.library_import.as_mut() else {
                return;
            };
            import.done += 1;
            import.finished()
        };
        if finished {
            let (total, failed) = self
                .library_import
                .take()
                .map(|import| (import.total, import.failed))
                .unwrap_or((0, 0));
            self.refresh_library_states();
            if failed > 0 {
                self.flash(format!(
                    "已导入 {} 个素材，{failed} 个读不出信息",
                    total - failed
                ));
            } else {
                self.flash(format!("已导入 {total} 个素材"));
            }
        }
        cx.notify();
    }
}
