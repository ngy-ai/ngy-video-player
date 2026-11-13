//! 菜单栏数据：条目构造与动作分发。

use super::*;

mod entries;

impl PlayerApp {
    /// 能不能对当前文件动刀（裁切、换轨道）：得有一个本地文件，且时长已经读出来。
    fn file_editable(&self) -> bool {
        self.snapshot.path.is_some() && self.snapshot.duration > 0.0
    }

    /// 有工程就是剪辑模式：两套菜单只靠这一个状态分流。
    fn editing(&self) -> bool {
        self.project.is_some()
    }

    /// 换模式时收掉另一套菜单里还开着的标题：拖入工程、关闭工程都不一定经过
    /// 菜单点击，留一个当前模式画不出来的 open 状态会让 hover 切换表现得很怪。
    pub(super) fn sync_menu_bar_mode(&mut self) {
        let menus = BarMenu::for_mode(self.editing());
        if self.menu_bar.is_some_and(|open| !menus.contains(&open)) {
            self.menu_bar = None;
        }
    }

    pub(super) fn bar_entries(&self, menu: BarMenu, window: &Window) -> Vec<BarItem> {
        use BarAction::*;

        let mut items: Vec<BarItem> = Vec::new();
        match menu {
            BarMenu::File => {
                // 两套「文件」菜单完全按模式分开：播放模式只管打开片子和进出工程，
                // 剪辑模式只管工程。交叉的入口（打开工程 / 关闭工程）各留在自己那一套。
                if self.editing() {
                    self.editing_file_entries(&mut items);
                } else {
                    self.playback_file_entries(&mut items);
                }
            }
            BarMenu::Playback => {
                items.push(BarItem::Entry(
                    BarEntry::new(
                        if self.snapshot.paused {
                            "播放"
                        } else {
                            "暂停"
                        },
                        Command(crate::player::Command::PlayPause),
                    )
                    .hint("空格"),
                ));
                items.push(BarItem::Entry(BarEntry::new(
                    "停止",
                    Command(crate::player::Command::Stop),
                )));
                items.push(BarItem::Separator);

                let navigable = self.playlist().len() > 1;
                items.push(BarItem::Entry(
                    BarEntry::new("上一集", Previous)
                        .hint("P")
                        .enabled(navigable),
                ));
                items.push(BarItem::Entry(
                    BarEntry::new("下一集", Next).hint("N").enabled(navigable),
                ));
                items.push(BarItem::Separator);
                items.push(BarItem::Entry(
                    BarEntry::new("快退 5 秒", Seek(-SEEK_STEP)).hint("←"),
                ));
                items.push(BarItem::Entry(
                    BarEntry::new("快进 5 秒", Seek(SEEK_STEP)).hint("→"),
                ));
                items.push(BarItem::Separator);

                for mode in [LoopMode::Off, LoopMode::All, LoopMode::One] {
                    items.push(BarItem::Entry(
                        BarEntry::new(mode.label(), SetLoop(mode))
                            .checked(self.settings.loop_mode == mode),
                    ));
                }

                items.push(BarItem::Separator);
                // A-B 循环：标记只对当前文件有效，换片会自动失效。
                let ab_start = self.ab_loop.start;
                let ab_end = self.ab_loop.end;
                items.push(BarItem::Entry(
                    BarEntry::new("标记 A-B 起点", MarkAbStart)
                        .checked(ab_start.is_some())
                        .enabled(self.file_editable()),
                ));
                items.push(BarItem::Entry(
                    BarEntry::new("标记 A-B 终点", MarkAbEnd)
                        .checked(ab_end.is_some())
                        .enabled(ab_start.is_some()),
                ));
                if let (Some(start), Some(end)) = (ab_start, ab_end) {
                    items.push(BarItem::Entry(BarEntry::note(format!(
                        "A-B：{} → {}",
                        format_time(start),
                        format_time(end)
                    ))));
                } else if let Some(start) = ab_start {
                    items.push(BarItem::Entry(BarEntry::note(format!(
                        "A-B：{} → 待标记",
                        format_time(start)
                    ))));
                }
                items.push(BarItem::Entry(
                    BarEntry::new("清除 A-B 循环", ClearAbLoop).enabled(ab_start.is_some()),
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
                items.push(BarItem::Entry(
                    BarEntry::new("镜头分割…", ShowScenes)
                        .enabled(self.file_editable() && self.snapshot.has_video()),
                ));
            }
            // 「编辑」两套内容：播放模式是单文件工具，剪辑模式是时间线命令。
            BarMenu::Edit => {
                if self.editing() {
                    self.edit_entries(&mut items);
                } else {
                    self.playback_edit_entries(&mut items);
                }
            }
            // 预览只在剪辑模式有：播放模式的播放控制在「播放」菜单里。
            BarMenu::Preview => {
                if self.editing() {
                    self.preview_entries(&mut items);
                }
            }
            BarMenu::Subtitle => {
                items.push(BarItem::Entry(
                    BarEntry::new(
                        "关闭字幕",
                        Command(crate::player::Command::SelectTrack {
                            kind: TrackKind::Subtitle,
                            id: -1,
                        }),
                    )
                    .checked(self.snapshot.selected_track(TrackKind::Subtitle).is_none()),
                ));

                let tracks = self.snapshot.track(TrackKind::Subtitle);
                if !tracks.is_empty() {
                    items.push(BarItem::Separator);
                }
                for track in tracks {
                    items.push(BarItem::Entry(
                        BarEntry::new(
                            track.display_name(),
                            Command(crate::player::Command::SelectTrack {
                                kind: TrackKind::Subtitle,
                                id: track.id,
                            }),
                        )
                        .checked(track.selected),
                    ));
                }

                items.push(BarItem::Separator);
                // 外挂字幕与片源差几秒是最常见的毛病，微调就放在字幕菜单里。
                items.push(BarItem::Entry(BarEntry::note(
                    if self.subtitle_delay.abs() < 0.001 {
                        "字幕延迟 0.0s".to_string()
                    } else {
                        format!("字幕延迟 {:+.1}s", self.subtitle_delay)
                    },
                )));
                items.push(BarItem::Entry(
                    BarEntry::new("字幕提前 0.1 秒", SubtitleDelay(-0.1)).hint("Z"),
                ));
                items.push(BarItem::Entry(
                    BarEntry::new("字幕延后 0.1 秒", SubtitleDelay(0.1)).hint("X"),
                ));
                items.push(BarItem::Entry(BarEntry::new(
                    "字幕延迟归零",
                    SubtitleDelay(0.0),
                )));
                items.push(BarItem::Separator);
                items.push(BarItem::Entry(BarEntry::new(
                    "从文件加载字幕…",
                    LoadSubtitle,
                )));
                items.push(BarItem::Entry(
                    BarEntry::new("内嵌字幕到文件…", EditSubtitle).enabled(self.file_editable()),
                ));
            }
            BarMenu::Audio => {
                let tracks = self.snapshot.track(TrackKind::Audio);
                if tracks.is_empty() {
                    items.push(BarItem::Entry(BarEntry::note("没有可切换的音轨")));
                }
                for track in tracks {
                    items.push(BarItem::Entry(
                        BarEntry::new(
                            track.display_name(),
                            Command(crate::player::Command::SelectTrack {
                                kind: TrackKind::Audio,
                                id: track.id,
                            }),
                        )
                        .checked(track.selected),
                    ));
                }
                items.push(BarItem::Separator);
                items.push(BarItem::Entry(
                    BarEntry::new("替换或删除音轨…", EditAudio).enabled(self.file_editable()),
                ));
            }
            BarMenu::View => {
                // 工程模式右侧通道整条让给编辑历史侧栏，播放列表根本没有可画的地方。
                if !self.editing() {
                    items.push(BarItem::Entry(
                        BarEntry::new("播放列表", TogglePlaylist)
                            .hint("L")
                            .checked(self.settings.show_playlist),
                    ));
                }
                items.push(BarItem::Entry(
                    BarEntry::new("全屏", ToggleFullscreen)
                        .hint("F")
                        .checked(window.is_fullscreen()),
                ));
                items.push(BarItem::Entry(
                    BarEntry::new("小窗", ToggleMini)
                        .hint("T")
                        .checked(self.mini.is_some()),
                ));
                items.push(BarItem::Separator);
                // 置顶要拿原生窗口句柄：Wayland 没给普通窗口留这个能力，只能置灰并说明。
                let on_top = platform::topmost::supported(window);
                for mode in OnTop::ALL {
                    items.push(BarItem::Entry(
                        BarEntry::new(mode.label(), SetOnTop(mode))
                            .checked(self.settings.on_top == mode)
                            .enabled(on_top),
                    ));
                }
                if !on_top {
                    items.push(BarItem::Entry(BarEntry::note("当前窗口系统不支持置顶")));
                }
            }
            BarMenu::Settings => {
                let supported = platform::associations::supported();
                let status = &self.association_status;

                // 所有偏好都收在同一个窗口里，菜单只留一个入口；分页功能在窗口左侧。
                items.push(BarItem::Entry(
                    BarEntry::new("设置…", OpenSettings(None)).checked(self.show_settings),
                ));
                // 剪片时最常调的是缓存，给个直达入口省得先开窗口再找页签。
                items.push(BarItem::Entry(BarEntry::new(
                    "渲染缓存…",
                    OpenSettings(Some(SettingsTab::Cache)),
                )));
                items.push(BarItem::Separator);
                items.push(BarItem::Entry(
                    BarEntry::new("设为默认视频播放器…", SetAsDefaultPlayer).enabled(supported),
                ));
                if cfg!(windows) {
                    items.push(BarItem::Entry(
                        BarEntry::new("打开系统默认应用设置", OpenDefaultApps).enabled(supported),
                    ));
                }
                items.push(BarItem::Separator);
                items.push(BarItem::Entry(
                    BarEntry::new("移除本程序的文件关联", RemoveAssociations)
                        .enabled(status.is_registered()),
                ));
                items.push(BarItem::Separator);
                items.push(BarItem::Entry(
                    BarEntry::new("启动时自动维护关联", ToggleMaintainAssociations)
                        .checked(self.settings.maintain_associations)
                        .enabled(supported),
                ));
                items.push(BarItem::Separator);
                items.push(BarItem::Entry(BarEntry::note(association_status_line(
                    status, supported,
                ))));
            }
            BarMenu::Help => {
                items.push(BarItem::Entry(
                    BarEntry::new("键盘快捷键", ToggleHelp)
                        .hint("?")
                        .checked(self.show_settings && self.settings_tab == SettingsTab::Shortcuts),
                ));
                items.push(BarItem::Separator);
                items.push(BarItem::Entry(
                    BarEntry::new("关于 ngy-video-player", About)
                        .checked(self.show_settings && self.settings_tab == SettingsTab::About),
                ));
            }
        }

        items
    }

    pub(super) fn run_bar_action(
        &mut self,
        action: BarAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu_bar = None;
        match action {
            BarAction::Command(command) => self.send(command),
            BarAction::OpenFile => self.pick_file(window, cx),
            BarAction::OpenFolder => self.pick_folder(window, cx),
            BarAction::NewProject => self.request_project_action(AfterSaveConfirm::NewProject, cx),
            BarAction::OpenProject => self.pick_project(window, cx),
            BarAction::OpenRecentProject(path) => {
                self.remember_dir(&path);
                // 走同一道未保存确认：直接换掉当前工程和拖入一个工程是一样的。
                self.request_project_action(AfterSaveConfirm::OpenProject(path), cx);
            }
            BarAction::ImportToTimeline => self.import_to_timeline(window, cx),
            BarAction::ToggleLibrary => self.toggle_library(window, cx),
            BarAction::ImportAssets => self.pick_assets(window, cx),
            BarAction::ImportAssetFolder => self.pick_asset_folder(window, cx),
            BarAction::ImportPlaylistToTimeline => self.open_playlist_import_dialog(window, cx),
            BarAction::ShowHistory => self.open_history_panel(cx),
            BarAction::SequenceSettings => {
                self.request_sequence_dialog(SequencePurpose::Edit, Some(window), cx);
            }
            BarAction::ManageSequences => self.open_sequence_manager_dialog(window, cx),
            BarAction::Save => {
                self.save_project(Some(window), cx, None);
            }
            BarAction::SaveAs => self.save_project_as(Some(window), cx, None),
            BarAction::CollectAssets => self.collect_assets(cx),
            BarAction::CloseProject => {
                self.request_project_action(AfterSaveConfirm::CloseProject, cx)
            }
            BarAction::TogglePreview => self.toggle_play(cx),
            BarAction::JumpToTimecode => self.open_timecode_dialog(window, cx),
            BarAction::ExportSequence => self.open_export_dialog(window, cx),
            BarAction::ExportOtio => self.export_otio(window, cx),
            BarAction::ImportOtio => self.import_otio(window, cx),
            BarAction::Undo => self.undo_edit(cx),
            BarAction::Redo => self.redo_edit(cx),
            BarAction::SplitClip => self.split_at_playhead(cx),
            BarAction::DeleteClips => {
                // 全屏时时间线整条让给画面，选中看不见，不能盲删。
                if !window.is_fullscreen() {
                    self.delete_selected_clips(false, cx);
                }
            }
            BarAction::ToggleMarkerAtPlayhead => self.toggle_marker_at_playhead(cx),
            BarAction::MarkIn => self.set_in_point(cx),
            BarAction::MarkOut => self.set_out_point(cx),
            BarAction::AddVideoTrack => self.add_track(crate::timeline::TrackKind::Video, cx),
            BarAction::AddAudioTrack => self.add_track(crate::timeline::TrackKind::Audio, cx),
            BarAction::AddSubtitleTrack => self.add_track(crate::timeline::TrackKind::Subtitle, cx),
            BarAction::AddSubtitle => {
                let start = self
                    .project
                    .as_ref()
                    .map(|project| project.ui.playhead)
                    .unwrap_or(crate::timeline::Tick::ZERO);
                self.open_subtitle_dialog(None, None, start, window, cx);
            }
            BarAction::ImportSubtitles => self.import_subtitle_files(window, cx),
            BarAction::ExportSubtitles => self.export_subtitle_file(window, cx),
            BarAction::LoadSubtitle => self.pick_subtitle(window, cx),
            BarAction::Previous => self.previous_file(cx),
            BarAction::Next => self.next_file(cx),
            BarAction::Seek(delta) => self.seek_by(delta, cx),
            BarAction::SetLoop(mode) => self.set_loop(mode, cx),
            BarAction::SetOnTop(mode) => self.set_on_top(mode),
            BarAction::TogglePlaylist => self.toggle_playlist(window),
            BarAction::ToggleFullscreen => self.toggle_fullscreen(window),
            BarAction::ToggleMini => self.toggle_mini(window, cx),
            BarAction::ToggleHelp => self.toggle_settings(SettingsTab::Shortcuts),
            BarAction::OpenSettings(tab) => self.open_settings(tab),
            BarAction::TakeScreenshot => self.take_screenshot(cx),
            BarAction::Trim => self.open_clip_dialog(window, cx),
            BarAction::Convert => self.open_convert_dialog(cx),
            BarAction::MarkAbStart => self.mark_ab_start(cx),
            BarAction::MarkAbEnd => self.mark_ab_end(cx),
            BarAction::ClearAbLoop => self.clear_ab_loop(cx),
            BarAction::SubtitleDelay(delta) => self.set_subtitle_delay(delta, cx),
            BarAction::EditAudio => self.open_remux_dialog(EditTarget::Audio, window, cx),
            BarAction::EditSubtitle => self.open_remux_dialog(EditTarget::Subtitle, window, cx),
            BarAction::EditMetadata => self.open_metadata_dialog(window, cx),
            BarAction::MakeProxy => self.open_proxy_dialog(window, cx),
            BarAction::ShowScope => self.open_scope_dialog(cx),
            BarAction::ShowMixer => self.open_mixer_panel(cx),
            BarAction::ExportStems => self.export_audio_stems(window, cx),
            BarAction::ShowScenes => self.open_scenes_dialog(cx),
            BarAction::MulticamExport => self.open_multicam_dialog(cx),
            BarAction::ConcatExport => self.open_concat_dialog(cx),
            BarAction::SetAsDefaultPlayer => self.set_default_player(),
            BarAction::OpenDefaultApps => self.open_default_apps(),
            BarAction::RemoveAssociations => self.remove_associations(window, cx),
            BarAction::ToggleMaintainAssociations => self.toggle_maintain_associations(),
            // 版本、引擎状态、配置位置这些信息一条提示条装不下，直接开模态窗口。
            BarAction::About => self.open_settings(Some(SettingsTab::About)),
            // 退出前要是不问一句，未保存的工程就跟着进程一起没了。
            BarAction::Quit => {
                if self.project.is_none() || !self.project_modified() {
                    cx.quit();
                    return;
                }
                self.request_project_action(AfterSaveConfirm::Quit, cx);
                return;
            }
        }
        self.poke();
        cx.notify();
    }

    // -- 文件关联 ----------------------------------------------------------
}

impl PlayerApp {
    /// 当前序列有没有音频轨。混音台与分轨导出都要求至少一条。
    pub(super) fn has_audio_track(&self) -> bool {
        self.project.as_ref().is_some_and(|project| {
            project
                .sequence()
                .tracks
                .iter()
                .any(|track| track.kind == crate::timeline::TrackKind::Audio)
        })
    }
}
