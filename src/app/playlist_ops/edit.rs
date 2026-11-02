//! 列表编辑：选中、折叠、删除，以及行上的右键菜单。

use super::super::*;

impl PlayerApp {
    // -- 列表编辑 ----------------------------------------------------------

    /// 列表行的单击语义：普通点击播放，Ctrl/⌘ 点击增删选择，Shift 点击连选。
    ///
    /// 没有单独的「选择模式」开关——播放器里点列表的第一意图永远是换歌，
    /// 把多选藏在修饰键后面比让每次点击都先进选择态更顺手。
    pub(in crate::app) fn select_item(
        &mut self,
        index: usize,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        // 文件夹行的普通点击是展开/收起：它没有东西可播，拿去换歌只会
        // 变成「点一下没反应」。多选仍然要能选到文件夹，整棵一起删。
        let is_dir = self
            .playlist()
            .items()
            .get(index)
            .is_some_and(|item| item.is_dir());
        if is_dir && !modifiers.shift && !modifiers.secondary() {
            self.toggle_dir(index, cx);
            return;
        }

        if modifiers.shift {
            let anchor = self.selection_anchor.unwrap_or(index);
            let (start, end) = if anchor <= index {
                (anchor, index)
            } else {
                (index, anchor)
            };
            // 连选替换上一次的区间，否则反复 Shift 点击只会越选越多、缩不回去。
            self.selection = (start..=end).collect();
            if self.selection_anchor.is_none() {
                self.selection_anchor = Some(index);
            }
            self.poke();
            cx.notify();
            return;
        }

        if modifiers.secondary() {
            if !self.selection.remove(&index) {
                self.selection.insert(index);
            }
            self.selection_anchor = Some(index);
            self.poke();
            cx.notify();
            return;
        }

        self.selection.clear();
        self.selection_anchor = Some(index);
        self.play_index(index);
        self.poke();
        cx.notify();
    }

    /// 一键收起或展开所有文件夹。
    ///
    /// 收起之后列表会变得很短，之前选中的行可能已经不在界面上了，所以和
    /// 单个文件夹折叠时一样，顺手把选择和落点清掉。
    pub(in crate::app) fn set_all_folded(&mut self, folded: bool, cx: &mut Context<Self>) {
        if !self.playlist_mut().set_all_collapsed(folded) {
            self.flash(if folded {
                "文件夹已经全部收起"
            } else {
                "文件夹已经全部展开"
            });
            cx.notify();
            return;
        }
        self.selection.clear();
        self.selection_anchor = None;
        self.drop_spot = None;
        self.playlists.save();
        self.flash(if folded {
            "已收起所有文件夹"
        } else {
            "已展开所有文件夹"
        });
        cx.notify();
    }

    /// 标题栏那颗按钮：已经是「全收起」就展开，否则收起。
    pub(in crate::app) fn toggle_fold_all(&mut self, cx: &mut Context<Self>) {
        let folded = !self.playlist().all_collapsed();
        self.set_all_folded(folded, cx);
    }

    /// 展开或收起一个文件夹。
    pub(in crate::app) fn toggle_dir(&mut self, index: usize, cx: &mut Context<Self>) {
        if !self.playlist_mut().toggle_dir(index) {
            return;
        }
        // 收起会让一批行消失，旧的选择和落点都指向了不存在的位置。
        self.selection.clear();
        self.selection_anchor = None;
        self.drop_spot = None;
        self.playlists.save();
        cx.notify();
    }

    /// 删除当前页里的若干项。删文件夹会连同里面的项一起删。
    ///
    /// 只改列表，不动引擎：正在放的文件早就装进去解码了，删掉列表里的条目不该让
    /// 画面黑一下。页内位置由 `remove_many` 顺延到后继项，这一集放完从那里接着走。
    pub(in crate::app) fn remove_items(&mut self, indexes: &[usize], cx: &mut Context<Self>) {
        let tab = self.playlists.active();
        let removal = {
            let Some(list) = self.playlists.tab_mut(tab) else {
                return;
            };
            list.remove_many(indexes)
        };
        if removal.removed == 0 {
            return;
        }
        self.selection.clear();
        self.selection_anchor = None;
        self.playlists.save();
        self.flash(if removal.removed_files == removal.removed {
            format!("已移除 {} 个文件", removal.removed)
        } else {
            format!(
                "已移除 {} 项（含 {} 个文件）",
                removal.removed, removal.removed_files
            )
        });
        self.poke();
        cx.notify();
    }

    pub(in crate::app) fn remove_selection(&mut self, cx: &mut Context<Self>) {
        if self.selection.is_empty() {
            return;
        }
        let indexes: Vec<usize> = self.selection.iter().copied().collect();
        self.remove_items(&indexes, cx);
    }

    pub(in crate::app) fn clear_playlist(&mut self, cx: &mut Context<Self>) {
        if self.playlist().is_empty() {
            return;
        }
        let count = self.playlist().file_count();
        // 只清列表，播放和磁盘上的文件都不动，所以不做二次确认——重新拖进来就行。
        self.playlist_mut().clear();
        self.selection.clear();
        self.selection_anchor = None;
        self.playlists.save();
        self.flash(format!("已清空播放列表（{count} 个文件仍在磁盘上）"));
        cx.notify();
    }

    // -- 行右键菜单 --------------------------------------------------------

    /// 右键某一行：在鼠标位置弹出菜单。越界就往窗口里收，别飘到屏幕外。
    pub(in crate::app) fn open_item_menu(
        &mut self,
        index: usize,
        path: PathBuf,
        dir: bool,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let viewport = window.viewport_size();
        let max_x = (f32::from(viewport.width) - ITEM_MENU_WIDTH - 8.0).max(8.0);
        let max_y = (f32::from(viewport.height) - ITEM_MENU_HEIGHT - 8.0).max(8.0);
        let position = point(
            px(f32::from(position.x).clamp(8.0, max_x)),
            px(f32::from(position.y).clamp(8.0, max_y)),
        );

        self.item_menu = Some(ItemMenu {
            index,
            path,
            dir,
            position,
        });
        // 右键菜单和悬浮菜单/菜单栏互斥，免得两层浮层叠在一起。
        self.menu = None;
        self.menu_bar = None;
        self.poke();
        cx.notify();
    }

    pub(in crate::app) fn reveal_item(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.item_menu = None;
        match platform::reveal::reveal(&path) {
            // 文件管理器可能被别的窗口盖住，给一句提示免得以为没反应。
            Ok(()) => self.flash("已在文件管理器中定位"),
            Err(message) => self.flash(message),
        }
        cx.notify();
    }

    /// 打开「查看格式」。探测要 loadfile，放到后台线程里跑，结果用 oneshot 回来。
    pub(in crate::app) fn show_file_info(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.item_menu = None;
        self.file_info = Some(FileInfoPanel {
            path: path.clone(),
            state: InfoState::Loading,
        });

        let target = path.clone();
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::player::probe::probe(&target));
        });

        cx.spawn(async move |this, cx| {
            let result = rx
                .await
                .unwrap_or_else(|_| Err("探测线程意外退出".to_string()));
            let _ = this.update(cx, |this, cx| {
                // 探测期间面板可能被关掉，或者已经换成另一个文件：只认同一份。
                let Some(panel) = this.file_info.as_mut() else {
                    return;
                };
                if panel.path != path {
                    return;
                }
                panel.state = match result {
                    Ok(info) => InfoState::Ready(info),
                    Err(message) => InfoState::Failed(message),
                };
                cx.notify();
            });
        })
        .detach();

        cx.notify();
    }
}
