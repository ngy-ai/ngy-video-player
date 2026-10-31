//! 播放列表操作：打开文件/目录、标签页、选中与删除、拖放。

use super::*;

impl PlayerApp {
    /// Esc 的语义：一层一层往回退，而不是直接全关。
    pub(super) fn exit_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab_rename.is_some() {
            // 输入框自己会吃掉 Esc（见 `on_rename_key`），这里兜住「焦点不在
            // 根视图」的那一次，保证 Esc 总能关掉它。
            self.tab_rename = None;
        } else if self.clip.is_some() {
            // 输入框有焦点时 Esc 被 `on_clip_key` 截走（只退出焦点）；这里处理
            // 「焦点已经在播放器上」的那一次，把面板收起来。
            self.close_clip_dialog(cx);
        } else if self.show_settings {
            self.show_settings = false;
        } else if self.file_info.is_some() {
            self.file_info = None;
        } else if self.item_menu.is_some() {
            self.item_menu = None;
        } else if self.tab_menu.is_some() {
            self.tab_menu = None;
        } else if self.panel_menu.is_some() {
            self.panel_menu = None;
        } else if self.menu_bar.is_some() {
            self.menu_bar = None;
        } else if self.menu == Some(Menu::SpeedCustom) {
            // 自定义面板是倍速菜单的第二层，Esc 先退回预设列表。
            self.menu = Some(Menu::Speed);
        } else if self.menu.is_some() {
            self.menu = None;
        } else if self.screenshot_card.is_some() {
            // 卡片是最后一个浮上来的东西，Esc 先收它，再考虑关面板、退全屏。
            self.clear_screenshot_card();
        } else if self.mini.is_some() {
            // 小窗是「从主窗口分出去」的，Esc 表示收回来。
            self.dismiss_mini(cx);
        } else if self.settings.show_playlist {
            self.set_playlist_visible(false);
        } else if window.is_fullscreen() {
            window.toggle_fullscreen();
        }
        self.poke();
        cx.notify();
    }

    pub(super) fn flash(&mut self, message: impl Into<SharedString>) {
        let text = message.into();
        self.osd = Some((
            text.split('\n').map(|line| line.to_string()).collect(),
            Instant::now(),
        ));
    }

    pub(super) fn on_ended(&mut self, reason: EndReason, cx: &mut Context<Self>) {
        // `Stopped` 是「被 stop、或正在播的文件被新文件顶掉」，不是放完了。
        // 换歌 / 删除当前项都会顺带触发它，跟着往前走会白跳一个文件。
        if matches!(
            reason,
            EndReason::Error | EndReason::Stopped | EndReason::Quit
        ) {
            cx.notify();
            return;
        }
        // 自动续播跟着「正在播的那一页」走，不跟当前显示页：否则在列表 B 里
        // 翻看时，A 里的片子放完了会跳到 B 的队列里。
        let Some(tab) = self.playback_tab() else {
            // 队列被关掉/清空了，就让这一集自然放完，不再往后接。
            self.controls_forced = true;
            cx.notify();
            return;
        };
        match self.settings.loop_mode {
            LoopMode::One => {
                if let Some(path) = self.playlists.tab(tab).and_then(|p| p.current()).cloned() {
                    self.load_file(&path);
                }
            }
            LoopMode::All => {
                if let Some(next) = self.playlists.tab(tab).and_then(|p| p.next_index(true)) {
                    self.play_tab_index(tab, next);
                } else {
                    self.controls_forced = true;
                    cx.notify();
                }
            }
            LoopMode::Off => {
                if let Some(next) = self.playlists.tab(tab).and_then(|p| p.next_index(false)) {
                    self.play_tab_index(tab, next);
                } else {
                    self.controls_forced = true;
                    cx.notify();
                }
            }
        }
    }

    // -- 列表与打开 --------------------------------------------------------

    pub(super) fn open_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        // 先记目录再干活：这个文件夹里可能一个能播的文件都没有（下面会早退），
        // 但用户下次打开对话框还是想从这里开始。
        if let Some(first) = paths.first() {
            self.remember_dir(first);
        }
        // 只有「单独打开一个文件」才把同目录的其它集一起排队。打开的是文件夹
        // 时不能这么干：那个文件夹里可能就一个文件，替换成父目录会完全跑题。
        let single_file = paths.len() == 1 && !paths[0].is_dir();
        let mut entries: Vec<playlist::Item> = Vec::new();
        let mut focus: Option<PathBuf> = None;

        for path in paths {
            if path.is_dir() {
                // 文件夹连同子文件夹一起收进来，列表里留下能折叠的树。
                let mut tree = playlist::collect_tree(&path);
                focus = focus.or_else(|| {
                    tree.iter()
                        .find(|item| !item.is_dir())
                        .map(|item| item.path().to_path_buf())
                });
                entries.append(&mut tree);
            } else if formats::is_playlist(&path) {
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

        // 「打开」的结果一律并进默认列表，并切到那一页。页签是用户手动整理用的：
        // 打开动作不该覆盖当前正在看的这页，也不该再自动新开一页——双击文件
        // 启动走的也是这里，落点必须稳定。
        let tab = self.playlists.default_index();
        self.playlists.set_active(tab);
        self.playlist_mut().append_entries(entries);
        self.selection.clear();
        self.selection_anchor = None;
        let Some(path) = focus else {
            // 只并进来一些空文件夹：列表留着，正在放的片子不动。
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

    /// 播放**当前显示页**里的第 `index` 项。
    pub(super) fn play_index(&mut self, index: usize) {
        let tab = self.playlists.active();
        self.play_tab_index(tab, index);
    }

    /// 播放某一页里的第 `index` 项：页内位置、播放归属、引擎三者一起对齐。
    ///
    /// 文件夹行没有可播的东西，点它只会展开/收起（见 `select_item`），走到这里
    /// 直接忽略。
    pub(super) fn play_tab_index(&mut self, tab: usize, index: usize) {
        let Some(path) = self
            .playlists
            .tab(tab)
            .and_then(|playlist| playlist.playable_path(index))
            .cloned()
        else {
            return;
        };
        if let Some(playlist) = self.playlists.tab_mut(tab) {
            playlist.set_index(index);
        }
        self.playing_tab = Some(tab);
        self.load_file(&path);
        self.playlists.save();
    }

    /// 启动时恢复上次的列表。
    ///
    /// 装载当前项但先按下暂停：用户可能只是顺手点开了播放器，不该一上来就出声。
    pub(super) fn restore_session(&mut self) {
        let Some(path) = self.playlist().current().cloned() else {
            return;
        };
        if !path.exists() {
            // 文件可能在被拔掉的移动硬盘上：列表留着，但不在启动时糊一条错误横幅。
            log_warn!("上次的播放项已不存在，跳过自动装载: {}", path.display());
            return;
        }
        self.playing_tab = Some(self.playlists.active());
        self.load_file(&path);
        self.send(Command::SetPaused(true));
    }

    pub(super) fn load_file(&mut self, path: &std::path::Path) {
        let url = path.to_string_lossy().to_string();
        self.snapshot.position = 0.0;
        self.snapshot.duration = 0.0;
        self.snapshot.tracks.clear();
        self.error = None;
        // 换片：上一部的预览图和时间点全部作废，预览实例里那个文件也卸掉，
        // 否则它会一直占着文件和一路解码器。
        self.clear_preview();
        self.send(Command::Load { url, start: None });
        log_info!("打开 {}", path.display());
    }

    pub(super) fn next_file(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.playback_tab() else {
            return;
        };
        let wrap = self.settings.loop_mode != LoopMode::Off
            || self.playlists.tab(tab).is_some_and(|list| list.len() > 1);
        if let Some(index) = self
            .playlists
            .tab(tab)
            .and_then(|list| list.next_index(wrap))
        {
            self.play_tab_index(tab, index);
            cx.notify();
        }
    }

    pub(super) fn previous_file(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.playback_tab() else {
            return;
        };
        let wrap = self.settings.loop_mode != LoopMode::Off
            || self.playlists.tab(tab).is_some_and(|list| list.len() > 1);
        if let Some(index) = self
            .playlists
            .tab(tab)
            .and_then(|list| list.previous_index(wrap))
        {
            self.play_tab_index(tab, index);
            cx.notify();
        }
    }

    // -- 列表编辑 ----------------------------------------------------------

    /// 列表行的单击语义：普通点击播放，Ctrl/⌘ 点击增删选择，Shift 点击连选。
    ///
    /// 没有单独的「选择模式」开关——播放器里点列表的第一意图永远是换歌，
    /// 把多选藏在修饰键后面比让每次点击都先进选择态更顺手。
    pub(super) fn select_item(
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
    pub(super) fn set_all_folded(&mut self, folded: bool, cx: &mut Context<Self>) {
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
    pub(super) fn toggle_fold_all(&mut self, cx: &mut Context<Self>) {
        let folded = !self.playlist().all_collapsed();
        self.set_all_folded(folded, cx);
    }

    /// 展开或收起一个文件夹。
    pub(super) fn toggle_dir(&mut self, index: usize, cx: &mut Context<Self>) {
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
    pub(super) fn remove_items(&mut self, indexes: &[usize], cx: &mut Context<Self>) {
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

    pub(super) fn remove_selection(&mut self, cx: &mut Context<Self>) {
        if self.selection.is_empty() {
            return;
        }
        let indexes: Vec<usize> = self.selection.iter().copied().collect();
        self.remove_items(&indexes, cx);
    }

    pub(super) fn clear_playlist(&mut self, cx: &mut Context<Self>) {
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

    // -- 多列表 ------------------------------------------------------------

    pub(super) fn add_tab(&mut self, cx: &mut Context<Self>) {
        let name = self.playlists.auto_name();
        self.playlists.add(name);
        self.selection.clear();
        self.selection_anchor = None;
        self.playlists.save();
        self.poke();
        cx.notify();
    }

    /// 切页不动播放：面板同时兼任「浏览」的职责，切过去就换歌太打扰。
    pub(super) fn select_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        self.playlists.set_active(index);
        self.selection.clear();
        self.selection_anchor = None;
        self.playlists.save();
        self.poke();
        cx.notify();
    }

    pub(super) fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        let name = self
            .playlists
            .tabs()
            .get(index)
            .map(|tab| tab.name.clone())
            .unwrap_or_default();
        // 默认页不画关闭按钮，菜单里那两项也是灰的；这里是最后一道闸，
        // 少一条路绕过「不能删」的约定。
        if self.playlists.is_default(index) {
            self.flash("「默认」列表不能删除");
            cx.notify();
            return;
        }
        if !self.playlists.remove(index) {
            return;
        }
        // 关页面不动播放：正在放的东西还在引擎里，让它自然放完，只是不再有「下一集」。
        if self.playing_tab == Some(index) {
            self.playing_tab = None;
        } else if let Some(playing) = self.playing_tab
            && playing > index
        {
            self.playing_tab = Some(playing - 1);
        }
        self.selection.clear();
        self.selection_anchor = None;
        self.playlists.save();
        self.flash(format!("已关闭列表「{name}」"));
        self.poke();
        cx.notify();
    }

    /// 页签右键：在鼠标位置弹出菜单。越界就往窗口里收，别飘到屏幕外。
    pub(super) fn open_tab_menu(
        &mut self,
        index: usize,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.playlists.len() {
            return;
        }
        let viewport = window.viewport_size();
        let max_x = (f32::from(viewport.width) - TAB_MENU_WIDTH - 8.0).max(8.0);
        let max_y = (f32::from(viewport.height) - TAB_MENU_HEIGHT - 8.0).max(8.0);
        let position = point(
            px(f32::from(position.x).clamp(8.0, max_x)),
            px(f32::from(position.y).clamp(8.0, max_y)),
        );

        self.tab_menu = Some(TabMenu { index, position });
        // 同一时刻只留一层右键菜单。
        self.item_menu = None;
        self.panel_menu = None;
        self.menu = None;
        self.menu_bar = None;
        self.poke();
        cx.notify();
    }

    /// 打开重命名输入框，预填当前名字。
    pub(super) fn begin_rename(&mut self, index: usize, cx: &mut Context<Self>) {
        self.tab_menu = None;
        // 默认列表的名字是程序的一部分（「打开」的结果放在这一页），不给改。
        if self.playlists.is_default(index) {
            self.flash("「默认」列表的名字固定，不能改名");
            cx.notify();
            return;
        }
        let Some(tab) = self.playlists.tabs().get(index) else {
            return;
        };
        let text = tab.name.clone();
        self.tab_rename = Some(TabRename {
            index,
            text,
            typed: false,
        });
        self.poke();
        cx.notify();
    }

    /// 确认改名。名字为空时保留原名，等于什么也没发生。
    pub(super) fn finish_rename(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.tab_rename.take() else {
            return;
        };
        let name = dialog.text.trim().to_string();
        if name.is_empty() {
            // 空名字会让页签看起来像丢了标题；退回原名，比留个空白页签好。
            self.flash("列表名不能为空，已保留原名");
            cx.notify();
            return;
        }
        let Some(current) = self
            .playlists
            .tabs()
            .get(dialog.index)
            .map(|tab| tab.name.clone())
        else {
            // 页签在输入过程中被关掉了：没有可改的对象，静默收场。
            cx.notify();
            return;
        };
        if current == name {
            cx.notify();
            return;
        }
        if !self.playlists.rename(dialog.index, name.clone()) {
            // 只剩两种原因：改的是默认列表，或者想给别的页起「默认」这个名字。
            self.flash("「默认」是保留名，不能改也不能重名");
            cx.notify();
            return;
        }
        self.playlists.save();
        self.flash(format!("已改名为「{name}」"));
        self.poke();
        cx.notify();
    }

    /// 重命名输入框的键盘：Enter 确认、Esc 取消、退格删除、其余可打印字符追加。
    pub(super) fn on_rename_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if self.tab_rename.is_none() {
            return false;
        }
        let modifiers = event.keystroke.modifiers;
        // 粘贴：输入框走的是按键转换，中文字符进不来（没有输入法处理器），
        // 想用中文名只能从别处复制进来。
        if (modifiers.control || modifiers.platform) && event.keystroke.key == "v" {
            if let Some(text) = cx
                .read_from_clipboard()
                .and_then(|item| item.text().map(|text| text.to_string()))
            {
                if let Some(dialog) = self.tab_rename.as_mut() {
                    let clipped: String = text
                        .chars()
                        .filter(|ch| !ch.is_control())
                        .take(MAX_TAB_NAME_CHARS)
                        .collect();
                    dialog.text = clipped.trim().to_string();
                    dialog.typed = true;
                }
                cx.notify();
            }
            return true;
        }
        // 其余组合键留给播放器：输入框只吃纯文本和编辑键。
        if modifiers.control || modifiers.platform || modifiers.alt {
            return false;
        }

        match event.keystroke.key.as_str() {
            "enter" => {
                self.finish_rename(cx);
                true
            }
            "escape" => {
                self.tab_rename = None;
                cx.notify();
                true
            }
            "backspace" => {
                if let Some(dialog) = self.tab_rename.as_mut() {
                    // 退格也算「动过键盘」：之后不能再把预填的名字整个清掉。
                    dialog.typed = true;
                    dialog.text.pop();
                }
                cx.notify();
                true
            }
            _ => {
                let Some(typed) = event.keystroke.key_char.as_deref() else {
                    // 方向键、功能键这些没有字符的按键：对话框开着时一律吞掉。
                    // 漏下去会让方向键去 seek，用户以为自己只是在挪光标。
                    return true;
                };
                if typed.is_empty() || typed.chars().any(|ch| ch.is_control()) {
                    return false;
                }
                if let Some(dialog) = self.tab_rename.as_mut() {
                    // 第一次敲字就整个替换：预填只是告诉用户正在改哪一页。
                    if !dialog.typed {
                        dialog.text.clear();
                        dialog.typed = true;
                    }
                    if dialog.text.chars().count() < MAX_TAB_NAME_CHARS {
                        dialog.text.push_str(typed);
                    }
                }
                cx.notify();
                true
            }
        }
    }

    // -- 行右键菜单 --------------------------------------------------------

    /// 右键某一行：在鼠标位置弹出菜单。越界就往窗口里收，别飘到屏幕外。
    pub(super) fn open_item_menu(
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

    pub(super) fn reveal_item(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.item_menu = None;
        match platform::reveal::reveal(&path) {
            // 文件管理器可能被别的窗口盖住，给一句提示免得以为没反应。
            Ok(()) => self.flash("已在文件管理器中定位"),
            Err(message) => self.flash(message),
        }
        cx.notify();
    }

    /// 打开「查看格式」。探测要 loadfile，放到后台线程里跑，结果用 oneshot 回来。
    pub(super) fn show_file_info(&mut self, path: PathBuf, cx: &mut Context<Self>) {
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

    pub(super) fn pick_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    pub(super) fn pick_add_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    pub(super) fn pick_add_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    pub(super) fn remember_dir(&mut self, path: &std::path::Path) {
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

    pub(super) fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn pick_subtitle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    // -- 帧 ----------------------------------------------------------------
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
