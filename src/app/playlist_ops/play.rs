//! 播放推进：点列表行换片、播完接下一集、启动时恢复上次的项。

use super::super::*;

impl PlayerApp {
    pub(in crate::app) fn on_ended(&mut self, reason: EndReason, cx: &mut Context<Self>) {
        // `Stopped` 是「被 stop、或正在播的文件被新文件顶掉」，不是放完了。
        // 换歌 / 删除当前项都会顺带触发它，跟着往前走会白跳一个文件。
        if matches!(
            reason,
            EndReason::Error | EndReason::Stopped | EndReason::Quit
        ) {
            cx.notify();
            return;
        }
        // 工程模式下 mpv 放的是时间线的预览素材：素材放到尾不等于序列放完，
        // 顺着播放列表接下一集会把时间线的画面顶掉。
        if self.project.is_some() {
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

    /// 播放**当前显示页**里的第 `index` 项。
    pub(in crate::app) fn play_index(&mut self, index: usize) {
        let tab = self.playlists.active();
        self.play_tab_index(tab, index);
    }

    /// 播放某一页里的第 `index` 项：页内位置、播放归属、引擎三者一起对齐。
    ///
    /// 文件夹行没有可播的东西，点它只会展开/收起（见 `select_item`），走到这里
    /// 直接忽略。
    pub(in crate::app) fn play_tab_index(&mut self, tab: usize, index: usize) {
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
    pub(in crate::app) fn restore_session(&mut self) {
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

    pub(in crate::app) fn load_file(&mut self, path: &std::path::Path) {
        let url = path.to_string_lossy().to_string();
        self.snapshot.position = 0.0;
        self.snapshot.duration = 0.0;
        self.snapshot.tracks.clear();
        self.snapshot.video_width = 0;
        self.snapshot.video_height = 0;
        self.error = None;
        // 换片：上一部的画面、预览图和时间点全部作废，预览实例里那个文件也卸掉，
        // 否则它会一直占着文件和一路解码器。引擎随后发来的 FrameCleared
        // 会同步清掉小窗的画面，并挡住通道里迟到的旧帧。
        self.begin_load();
        self.clear_preview();
        self.send(Command::Load { url, start: None });
        // 字幕延迟是逐文件选项：换片后 mpv 会把它归零，界面这一份也得跟着清，
        // 否则菜单里显示的还是上一部片子的数字。这里再显式发一次 0，不赌默认行为。
        self.subtitle_delay = 0.0;
        self.send(Command::SetOption {
            name: "sub-delay".to_string(),
            value: "0".to_string(),
        });
        log_info!("打开 {}", path.display());
    }

    pub(in crate::app) fn next_file(&mut self, cx: &mut Context<Self>) {
        // 工程模式的播放源是时间线，换片只会把预览素材顶掉（列表在工程模式下也不画）。
        if self.project.is_some() {
            return;
        }
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

    pub(in crate::app) fn previous_file(&mut self, cx: &mut Context<Self>) {
        if self.project.is_some() {
            return;
        }
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
}
