//! 播放列表操作：打开文件/目录、标签页、选中与删除、拖放。

use super::*;

impl PlayerApp {
    /// Esc 的语义：一层一层往回退，而不是直接全关。
    pub(super) fn exit_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.show_help {
            self.show_help = false;
        } else if self.show_picture {
            self.show_picture = false;
        } else if self.show_engine {
            self.show_engine = false;
        } else if self.file_info.is_some() {
            self.file_info = None;
        } else if self.item_menu.is_some() {
            self.item_menu = None;
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
        let mut files: Vec<PathBuf> = Vec::new();
        let mut focus: Option<PathBuf> = None;
        // 单独打开一个 .m3u/.pls 时新开一页：它本来就是一份独立列表，
        // 直接覆盖当前页会把用户正在看的那一页顶掉。
        let as_new_tab = paths.len() == 1 && formats::is_playlist(&paths[0]);
        let label = tab_name_for(&paths);

        for path in paths {
            if path.is_dir() {
                let mut collected = playlist::collect_dir(&path);
                focus = focus.or_else(|| collected.first().cloned());
                files.append(&mut collected);
            } else if formats::is_playlist(&path) {
                let mut entries = playlist::parse_playlist(&path);
                focus = focus.or_else(|| entries.first().cloned());
                files.append(&mut entries);
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
                files.push(path.clone());
            }
        }

        if files.is_empty() {
            self.flash("没有找到可播放的文件");
            cx.notify();
            return;
        }

        // 只拖进来一个文件时，把同目录的其它媒体一起排队 —— 看剧集时这是默认期待。
        let single = files.len() == 1;
        if single {
            let path = files[0].clone();
            focus = Some(path.clone());
            if let Some(parent) = path.parent() {
                let mut siblings = playlist::collect_dir(parent);
                if siblings.len() > 1 {
                    files = std::mem::take(&mut siblings);
                }
            }
        }

        let start = focus
            .as_ref()
            .and_then(|target| files.iter().position(|path| path == target))
            .unwrap_or(0);

        if let Some(path) = &focus {
            self.settings.last_dir = path.parent().map(|p| p.to_path_buf());
        }

        // 当前页本来就是空的就别再新开一页，否则启动时那个空页会一直留着。
        let reuse_empty = self.playlist().is_empty();
        if as_new_tab && !reuse_empty {
            let name = label.unwrap_or_else(|| self.playlists.auto_name());
            self.playlists.add(name);
        } else if reuse_empty {
            // 空页借这次打开的文件起个名字，省得列表全叫「列表 N」。
            if let Some(name) = label {
                self.playlists.rename_active(name);
            }
        }
        self.playlist_mut().replace(files, start);
        self.selection.clear();
        self.selection_anchor = None;
        self.play_index(start);
    }

    /// 播放**当前显示页**里的第 `index` 项。
    pub(super) fn play_index(&mut self, index: usize) {
        let tab = self.playlists.active();
        self.play_tab_index(tab, index);
    }

    /// 播放某一页里的第 `index` 项：页内位置、播放归属、引擎三者一起对齐。
    pub(super) fn play_tab_index(&mut self, tab: usize, index: usize) {
        {
            let Some(playlist) = self.playlists.tab_mut(tab) else {
                return;
            };
            playlist.set_index(index);
        }
        self.playing_tab = Some(tab);
        if let Some(path) = self.playlists.tab(tab).and_then(|p| p.get(index)).cloned() {
            self.load_file(&path);
        }
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

    /// 删除当前页里的若干项。
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
        self.flash(format!("已移除 {} 个文件", removal.removed));
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
        let count = self.playlist().len();
        if count == 0 {
            return;
        }
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

    // -- 行右键菜单 --------------------------------------------------------

    /// 右键某一行：在鼠标位置弹出菜单。越界就往窗口里收，别飘到屏幕外。
    pub(super) fn open_item_menu(
        &mut self,
        index: usize,
        path: PathBuf,
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

        let (label, extensions) = formats::dialog_filter();
        // 挂到主窗口上：对话框是模态的，打开期间主窗口不接受输入。
        // 过滤器给的是裸扩展名，`rfd` 自己补 `*.`；另外留一个「所有文件」，
        // 这样清单里没有的格式也还能手动翻出来打开（mpv 认得的多得多）。
        let mut dialog = rfd::FileDialog::new()
            .add_filter(&label, &extensions)
            .add_filter("视频文件", formats::VIDEO)
            .add_filter("音频文件", formats::AUDIO)
            .add_filter("所有文件", &["*"])
            .set_parent(window);
        if let Some(dir) = &self.settings.last_dir {
            dialog = dialog.set_directory(dir);
        }

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
        if let Some(path) = self.playlist().current() {
            if let Some(dir) = path.parent() {
                dialog = dialog.set_directory(dir);
            }
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
