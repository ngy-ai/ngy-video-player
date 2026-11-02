//! 「打开」与「添加」：文件对话框、目录展开、落进哪一页。

use super::super::*;

impl PlayerApp {
    // -- 列表与打开 --------------------------------------------------------

    pub(in crate::app) fn open_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        // 先记目录再干活：这个文件夹里可能一个能播的文件都没有（下面会早退），
        // 但用户下次打开对话框还是想从这里开始。
        if let Some(first) = paths.first() {
            self.remember_dir(first);
        }
        // 文件夹和文件走两条路：文件夹开成自己的一页列表，文件才并进默认页。
        // 拖入、菜单、命令行都从这里过，落点保持一致。
        let mut files = Vec::new();
        for path in paths {
            if path.is_dir() {
                self.open_folder(path, cx);
            } else {
                files.push(path);
            }
        }
        if !files.is_empty() {
            self.open_files(files, cx);
        }
    }

    /// 「打开文件夹」：新开一页播放列表，页名取文件夹名，装进整棵树并播放第一项。
    ///
    /// 和「添加文件夹」（[`Self::add_paths`]）分工明确：这里是「换一批来看」，
    /// 那边才是往当前这一页里补。同名页已经开着就切过去重装，连点两次不该
    /// 多出两页一模一样的页签。
    pub(in crate::app) fn open_folder(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        let entries = playlist::collect_tree(&folder);
        let focus = entries
            .iter()
            .find(|item| !item.is_dir())
            .map(|item| item.path().to_path_buf());
        let Some(focus) = focus else {
            // 空文件夹不占页签：一页什么都放不了，用户还得自己关掉。
            self.flash("这个文件夹里没有可播放的文件");
            cx.notify();
            return;
        };
        let name = folder
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| folder.to_string_lossy().into_owned());
        // 页名两头不留空白：`find_tab` 按名字复用，带空格的话每次打开都会新开一页。
        let name = name.trim().to_string();
        let tab = match self.playlists.find_tab(&name) {
            Some(tab) => tab,
            None => {
                let name = self.playlists.unique_name(&name);
                self.playlists.add(name)
            }
        };
        self.playlists.set_active(tab);
        self.playlist_mut().replace_entries(entries, Some(&focus));
        self.selection.clear();
        self.selection_anchor = None;
        // 打开就放：这一页的第一个可播项直接装进引擎。
        if let Some(index) = self.playlist().index() {
            self.play_index(index);
        }
        self.poke();
        cx.notify();
    }

    /// 打开一批文件 / m3u：并进默认页并播放焦点项。
    fn open_files(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        // 只有「单独打开一个文件」才把同目录的其它集一起排队。
        let single_file = paths.len() == 1;
        let mut entries: Vec<playlist::Item> = Vec::new();
        let mut focus: Option<PathBuf> = None;

        for path in paths {
            if formats::is_playlist(&path) {
                let mut items: Vec<playlist::Item> = playlist::parse_playlist(&path)
                    .into_iter()
                    .map(playlist::Item::file)
                    .collect();
                focus = focus.or_else(|| items.first().map(|item| item.path().to_path_buf()));
                entries.append(&mut items);
            } else if formats::is_subtitle(&path) {
                if let Some(engine) = &self.engine {
                    engine.open_subtitle(&path);
                    self.flash(format!(
                        "已加载字幕：{}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ));
                }
                cx.notify();
                return;
            } else {
                focus.get_or_insert_with(|| path.clone());
                entries.push(playlist::Item::file(path));
            }
        }

        if entries.is_empty() {
            self.flash("没有找到可播放的文件");
            cx.notify();
            return;
        }

        // 只拖进来一个文件时，把同目录的其它媒体一起排队 —— 看剧集时这是默认期待。
        if single_file && entries.len() == 1 {
            let path = entries[0].path().to_path_buf();
            focus = Some(path.clone());
            if let Some(parent) = path.parent() {
                let siblings = playlist::collect_dir(parent);
                if siblings.len() > 1 {
                    entries = siblings.into_iter().map(playlist::Item::file).collect();
                }
            }
        }

        // 打开文件的结果一律并进默认列表，并切到那一页。页签是用户手动整理用的：
        // 打开文件不该覆盖当前正在看的这页，也不该再自动新开一页——双击文件
        // 启动走的也是这里，落点必须稳定；文件夹在 `open_folder` 里另开一页。
        let tab = self.playlists.default_index();
        self.playlists.set_active(tab);
        self.playlist_mut().append_entries(entries);
        self.selection.clear();
        self.selection_anchor = None;
        let Some(path) = focus else {
            // 保底：没有可播项时列表留着，正在放的片子不动。
            self.playlists.save();
            cx.notify();
            return;
        };
        // 焦点跟着刚打开的那一项走；它早就在列表里时也切过去重播，
        // 这样第二次双击同一个文件仍然有反应。
        self.playlist_mut().follow(Some(path));
        if let Some(index) = self.playlist().index() {
            self.play_index(index);
        }
    }

    pub(in crate::app) fn pick_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;
        let dialog = media_dialog(window, self.settings.last_dir.as_ref());

        // rfd 的对话框是阻塞的：扔到独立线程，结果通过 oneshot 回来。
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(dialog.pick_files());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten().unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if picked.is_empty() {
                    return;
                }
                this.open_paths(picked, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// 面板标题栏的「添加」：选几个文件追加到当前列表，不动正在放的那一集。
    ///
    /// 和「打开」是两条路：打开会换掉整份队列（那是「我要看这个」），
    /// 添加只是往队尾补（那是「待会儿再看」）。
    pub(in crate::app) fn pick_add_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;
        let dialog = media_dialog(window, self.settings.last_dir.as_ref());

        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(dialog.pick_files());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten().unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if picked.is_empty() {
                    return;
                }
                this.add_paths(picked, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// 选一个文件夹，把它里面能播的文件追加到当前列表。
    pub(in crate::app) fn pick_add_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let mut dialog = rfd::FileDialog::new().set_parent(window);
        if let Some(dir) = &self.settings.last_dir {
            dialog = dialog.set_directory(dir);
        }
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(dialog.pick_folder());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if let Some(folder) = picked {
                    this.add_paths(vec![folder], cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// 记住下次文件对话框从哪个目录打开。
    ///
    /// 记的是选中项的**父目录**：选了一个文件夹时，下次正好停在能看见它的位置。
    /// 如果记成文件夹自己，对话框会直接扎进去，只剩里面的内容——用户反而找不到
    /// 刚添加的那一层，看上去像「没记住」。
    pub(in crate::app) fn remember_dir(&mut self, path: &std::path::Path) {
        if let Some(parent) = dialog_dir(path) {
            self.settings.last_dir = Some(parent);
        }
    }

    /// 把文件追加到当前列表末尾。
    ///
    /// 列表本来是空的话顺手把第一项放起来：点完「添加」画面一点动静都没有，
    /// 会让人以为按钮没生效。
    pub(super) fn add_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if let Some(first) = paths.first() {
            self.remember_dir(first);
        }
        let was_empty = self.playlist().is_empty();
        let added = self.playlist_mut().append_paths(&paths);
        if added == 0 {
            self.flash("没有可添加的文件（可能已经在列表里了）");
            cx.notify();
            return;
        }
        // 第一项可能是文件夹行，从第一个真正能播的开始。
        if was_empty && let Some(index) = self.playlist().first_playable() {
            self.play_index(index);
        }
        self.playlists.save();
        self.flash(format!("已添加 {added} 个文件"));
        self.poke();
        cx.notify();
    }

    pub(in crate::app) fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let mut dialog = rfd::FileDialog::new().set_parent(window);
        if let Some(dir) = &self.settings.last_dir {
            dialog = dialog.set_directory(dir);
        }
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(dialog.pick_folder());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if let Some(folder) = picked {
                    this.open_paths(vec![folder], cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(in crate::app) fn pick_subtitle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let mut dialog = rfd::FileDialog::new()
            .add_filter("字幕文件", formats::SUBTITLE)
            .add_filter("所有文件", &["*"])
            .set_parent(window);
        if let Some(dir) = self.playlist().current().and_then(|path| path.parent()) {
            dialog = dialog.set_directory(dir);
        }
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(dialog.pick_file());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if let Some(path) = picked {
                    let url = path.to_string_lossy().to_string();
                    this.send(Command::AddSubtitle(url));
                    this.flash(format!(
                        "已加载字幕：{}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ));
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

/// 「打开」和「添加」共用同一套过滤器，省得两处的清单慢慢长歪。
///
/// 对话框挂在主窗口上：它是模态的，打开期间主窗口不接受输入。过滤器给的是裸扩展名，
/// `rfd` 自己补 `*.`；另外留一个「所有文件」，这样清单里没有的格式也还能手动翻出来
/// 打开（mpv 认得的多得多）。
fn media_dialog(window: &Window, dir: Option<&PathBuf>) -> rfd::FileDialog {
    let (label, extensions) = formats::dialog_filter();
    let mut dialog = rfd::FileDialog::new()
        .add_filter(&label, &extensions)
        .add_filter("视频文件", formats::VIDEO)
        .add_filter("音频文件", formats::AUDIO)
        .add_filter("所有文件", &["*"])
        .set_parent(window);
    if let Some(dir) = dir {
        dialog = dialog.set_directory(dir);
    }
    dialog
}

/// 选中项对应的「下次从这里打开」的目录。
///
/// 文件和文件夹一视同仁地取父目录。裸文件名（相对路径没有目录可言）返回 `None`，
/// 调用方保持原来的值不动——记一个空路径只会让对话框退回系统默认位置。
fn dialog_dir(path: &std::path::Path) -> Option<PathBuf> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(std::path::Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 记住的目录是选中项的父目录() {
        let folder = std::path::Path::new("/media/shows");
        assert_eq!(
            dialog_dir(folder),
            Some(PathBuf::from("/media")),
            "文件夹记它的上一层"
        );

        let episode = std::path::Path::new("/media/shows/ep1.mkv");
        assert_eq!(
            dialog_dir(episode),
            Some(PathBuf::from("/media/shows")),
            "文件记它所在的目录"
        );

        assert_eq!(
            dialog_dir(std::path::Path::new("ep1.mkv")),
            None,
            "裸文件名没有目录，不要覆盖已经记住的位置"
        );
    }
}
