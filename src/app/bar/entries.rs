//! 菜单条目：每套菜单各自列条目，ar.rs 只按当前模式分发。
//!
//! 这些是 PlayerApp 的固有方法、分散在子模块里，所以对父模块 pub(super)；
//! 条目一律直接读 self，不缓存——菜单每帧重建，缓存的条目会和工程状态脱节。

use super::*;

impl PlayerApp {
    /// 两套「文件」菜单共用的最近工程列表：前面永远留一条分隔线，有条目时
    /// 后面再补一条，免得和后面的工具 / 导入项挤在一起。
    pub(super) fn recent_projects(&self, items: &mut Vec<BarItem>) {
        use BarAction::*;

        items.push(BarItem::Separator);
        // 最近用过的工程：省掉一次文件对话框。菜单里只放前 5 条。
        for path in self.settings.recent_projects.iter().take(5) {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            items.push(BarItem::Entry(BarEntry::new(
                format!("最近：{name}"),
                OpenRecentProject(path.clone()),
            )));
        }
        if !self.settings.recent_projects.is_empty() {
            items.push(BarItem::Separator);
        }
    }

    /// 播放模式「文件」：只管打开片子、进出工程和退出。裁切 / 转换这类
    /// 单文件处理都收在同模式的「编辑」里，文件菜单不再混工具项。
    /// 工程的导入 / 保存 / 关闭一概不出现——那几个动作在没有工程时点了也没意义。
    pub(super) fn playback_file_entries(&self, items: &mut Vec<BarItem>) {
        use BarAction::*;

        items.push(BarItem::Entry(
            BarEntry::new("打开文件…", OpenFile).hint("O"),
        ));
        // 标出「新建列表」：面板里的「添加文件夹」是并进当前这一页，
        // 两个入口长得像，靠这一句区分。
        items.push(BarItem::Entry(BarEntry::new(
            "打开文件夹（新建列表）…",
            OpenFolder,
        )));
        items.push(BarItem::Separator);
        items.push(BarItem::Entry(BarEntry::new("新建工程", NewProject)));
        // 打开工程是播放模式进剪辑模式的入口，也是这套菜单里唯一的工程工作项。
        items.push(BarItem::Entry(BarEntry::new("打开工程…", OpenProject)));
        self.recent_projects(items);
        items.push(BarItem::Entry(BarEntry::new("退出", Quit)));
    }

    /// 播放模式「编辑」：针对当前文件的处理工具。名字沿用「编辑」，是因为
    /// 这些动作的产物都是编辑后的新文件；时间线编辑在剪辑模式的同名菜单里。
    pub(super) fn playback_edit_entries(&self, items: &mut Vec<BarItem>) {
        use BarAction::*;

        // 没有片子、或者时长还没读出来时无从编辑。
        items.push(BarItem::Entry(
            BarEntry::new("裁切片段…", Trim).enabled(self.file_editable()),
        ));
        // 没有正在播放的文件也能用：面板里可以直接选一个文件夹批量转。
        items.push(BarItem::Entry(BarEntry::new("格式转换…", Convert)));
        items.push(BarItem::Entry(
            BarEntry::new("生成代理文件…", MakeProxy)
                .enabled(self.file_editable() && self.snapshot.has_video()),
        ));
        items.push(BarItem::Separator);
        items.push(BarItem::Entry(
            BarEntry::new("替换或删除音轨…", EditAudio).enabled(self.file_editable()),
        ));
        items.push(BarItem::Entry(
            BarEntry::new("内嵌字幕…", EditSubtitle).enabled(self.file_editable()),
        ));
        items.push(BarItem::Entry(
            BarEntry::new("编辑章节与元数据…", EditMetadata).enabled(self.file_editable()),
        ));
        items.push(BarItem::Separator);
        items.push(BarItem::Entry(
            BarEntry::new("双机位并排导出…", MulticamExport)
                .enabled(self.file_editable() && self.snapshot.has_video()),
        ));
        items.push(BarItem::Entry(
            BarEntry::new("把列表拼成一个文件…", ConcatExport)
                .enabled(self.playlist().file_count() >= 2),
        ));
    }

    /// 剪辑模式「文件」：工程从新建到关闭的一整套工作项。打开单个片子 / 文件夹、
    /// 单文件工具都属于播放模式，这里一个都不放，免得把工程预览的素材当成片子换掉。
    pub(super) fn editing_file_entries(&self, items: &mut Vec<BarItem>) {
        use BarAction::*;

        items.push(BarItem::Entry(BarEntry::new("新建工程", NewProject)));
        items.push(BarItem::Entry(BarEntry::new("打开工程…", OpenProject)));
        self.recent_projects(items);
        // 分辨率与帧率建好之后还要能改：竖拍片子剪完想改回横屏是常事。
        items.push(BarItem::Entry(BarEntry::new("序列规格…", SequenceSettings)));
        items.push(BarItem::Entry(BarEntry::new(
            "导入到时间线…",
            ImportToTimeline,
        )));
        // 列表里已经排好的东西直接当素材用：先选列表，再选其中一个视频。
        let has_playlist_files = self
            .playlists
            .tabs()
            .iter()
            .any(|tab| tab.playlist.file_count() > 0);
        items.push(BarItem::Entry(
            BarEntry::new("从播放列表导入…", ImportPlaylistToTimeline).enabled(has_playlist_files),
        ));
        items.push(BarItem::Separator);
        // 撤销 / 重做只能一格一格挪，右侧历史侧栏能直接跳到任意一步。
        items.push(BarItem::Entry(
            BarEntry::new("编辑历史侧栏", ShowHistory).checked(self.show_history),
        ));
        // 有未保存改动时，标题栏会挂一个 `*`；这里把保存入口放在最顺手的位置。
        let save_hint = if cfg!(target_os = "macos") {
            "⌘S"
        } else {
            "Ctrl+S"
        };
        let save_as_hint = if cfg!(target_os = "macos") {
            "⇧⌘S"
        } else {
            "Ctrl+Shift+S"
        };
        items.push(BarItem::Entry(
            BarEntry::new("保存工程", Save).hint(save_hint),
        ));
        items.push(BarItem::Entry(
            BarEntry::new("工程另存为…", SaveAs).hint(save_as_hint),
        ));
        // 收集后工程变成自包含的：换机器打开不会缺素材。
        items.push(BarItem::Entry(
            BarEntry::new("收集素材到工程目录", CollectAssets).enabled(self.project_path.is_some()),
        ));
        items.push(BarItem::Separator);
        items.push(BarItem::Entry(BarEntry::new(
            "导出为视频文件…",
            ExportSequence,
        )));
        // 分轨导出是「不做插件宿主」的出路（ADR 0011）：要降噪 / 混响就把分轨拿去 DAW。
        items.push(BarItem::Entry(
            BarEntry::new("导出音频分轨…", ExportStems).enabled(self.has_audio_track()),
        ));
        // 与别的剪辑软件换手：OTIO 带得走轨道、片段、转场与标记。
        items.push(BarItem::Entry(BarEntry::new(
            "导出时间线为 OTIO…",
            ExportOtio,
        )));
        items.push(BarItem::Entry(BarEntry::new("从 OTIO 导入…", ImportOtio)));
        items.push(BarItem::Separator);
        items.push(BarItem::Entry(BarEntry::new("关闭工程", CloseProject)));
        items.push(BarItem::Separator);
        items.push(BarItem::Entry(BarEntry::new("退出", Quit)));
    }

    /// 剪辑模式「编辑」：时间线上的编辑命令，复用键盘快捷键对应的那批方法。
    pub(super) fn edit_entries(&self, items: &mut Vec<BarItem>) {
        use BarAction::*;

        // 一个工程可以有多条序列：新建 / 切换 / 改名 / 删除都在同一张面板里。
        items.push(BarItem::Entry(BarEntry::new("序列管理…", ManageSequences)));
        items.push(BarItem::Separator);
        let can_undo = self.history.as_ref().is_some_and(UndoHistory::can_undo);
        let can_redo = self.history.as_ref().is_some_and(UndoHistory::can_redo);
        // 素材库是「从零编排」的第一步：先收素材，再往轨道上放。
        items.push(BarItem::Entry(
            BarEntry::new("素材库", ToggleLibrary).checked(self.library.is_some()),
        ));
        items.push(BarItem::Entry(BarEntry::new("导入素材…", ImportAssets)));
        items.push(BarItem::Entry(BarEntry::new(
            "导入素材文件夹…",
            ImportAssetFolder,
        )));
        items.push(BarItem::Separator);
        items.push(BarItem::Entry(
            BarEntry::new("撤销", Undo)
                .hint(if cfg!(target_os = "macos") {
                    "⌘Z"
                } else {
                    "Ctrl+Z"
                })
                .enabled(can_undo),
        ));
        items.push(BarItem::Entry(
            BarEntry::new("重做", Redo)
                .hint(if cfg!(target_os = "macos") {
                    "⇧⌘Z"
                } else {
                    "Ctrl+Shift+Z"
                })
                .enabled(can_redo),
        ));
        items.push(BarItem::Separator);
        items.push(BarItem::Entry(
            BarEntry::new("在播放头处切割", SplitClip).hint(if cfg!(target_os = "macos") {
                "⌘K"
            } else {
                "Ctrl+K"
            }),
        ));
        let has_selection = self
            .project
            .as_ref()
            .is_some_and(|project| !project.ui.selected.is_empty());
        items.push(BarItem::Entry(
            BarEntry::new("删除选中片段", DeleteClips)
                .hint("Delete")
                .enabled(has_selection),
        ));
        items.push(BarItem::Entry(
            BarEntry::new("标记 / 取消标记", ToggleMarkerAtPlayhead).hint("Shift+M"),
        ));
        items.push(BarItem::Entry(
            BarEntry::new("设工作区入点", MarkIn).hint("Shift+I"),
        ));
        items.push(BarItem::Entry(
            BarEntry::new("设工作区出点", MarkOut).hint("Shift+O"),
        ));
        items.push(BarItem::Separator);
        items.push(BarItem::Entry(BarEntry::new("加视频轨", AddVideoTrack)));
        items.push(BarItem::Entry(BarEntry::new("加音频轨", AddAudioTrack)));
        items.push(BarItem::Entry(BarEntry::new("加字幕轨", AddSubtitleTrack)));
        // 混音台管的是「已经有的音频轨」：一条都没有时打开只是一句说明，先禁用更清楚。
        items.push(BarItem::Entry(
            BarEntry::new("混音台…", ShowMixer).enabled(self.has_audio_track()),
        ));
        items.push(BarItem::Entry(BarEntry::new(
            "在播放头加字幕…",
            AddSubtitle,
        )));
    }

    /// 剪辑模式「预览」：只留对工程预览有意义的动作。播放列表换集、循环、A-B
    /// 循环与镜头分割都作用于单个文件，工程模式下要么无效要么会打乱预览。
    pub(super) fn preview_entries(&self, items: &mut Vec<BarItem>) {
        use BarAction::*;

        items.push(BarItem::Entry(
            BarEntry::new(
                if self.timeline_playing {
                    "暂停预览"
                } else {
                    "播放预览"
                },
                TogglePreview,
            )
            .hint("空格"),
        ));
        items.push(BarItem::Separator);
        items.push(BarItem::Entry(
            BarEntry::new("快退 5 秒", Seek(-SEEK_STEP)).hint("←"),
        ));
        items.push(BarItem::Entry(
            BarEntry::new("快进 5 秒", Seek(SEEK_STEP)).hint("→"),
        ));
        items.push(BarItem::Entry(
            BarEntry::new("跳到时间码…", JumpToTimecode).hint(if cfg!(target_os = "macos") {
                "⌘G"
            } else {
                "Ctrl+G"
            }),
        ));
        items.push(BarItem::Separator);
        items.push(BarItem::Entry(
            BarEntry::new("截图", TakeScreenshot)
                .hint("S")
                .enabled(self.snapshot.has_video()),
        ));
        items.push(BarItem::Entry(
            BarEntry::new("示波器…", ShowScope).enabled(self.snapshot.has_video()),
        ));
    }
}
