//! 菜单栏数据：条目构造与动作分发。

use super::*;

impl PlayerApp {
    /// 能不能对当前文件动刀（裁切、换轨道）：得有一个本地文件，且时长已经读出来。
    fn file_editable(&self) -> bool {
        self.snapshot.path.is_some() && self.snapshot.duration > 0.0
    }

    pub(super) fn bar_entries(&self, menu: BarMenu, window: &Window) -> Vec<BarItem> {
        use BarAction::*;

        let mut items: Vec<BarItem> = Vec::new();
        match menu {
            BarMenu::File => {
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
                // 没有片子、或者时长还没读出来时无从编辑。
                items.push(BarItem::Entry(
                    BarEntry::new("裁切片段…", Trim).enabled(self.file_editable()),
                ));
                // 没有正在播放的文件也能用：面板里可以直接选一个文件夹批量转。
                items.push(BarItem::Entry(BarEntry::new("格式转换…", Convert)));
                items.push(BarItem::Entry(
                    BarEntry::new("替换或删除音轨…", EditAudio).enabled(self.file_editable()),
                ));
                items.push(BarItem::Entry(
                    BarEntry::new("内嵌字幕…", EditSubtitle).enabled(self.file_editable()),
                ));
                items.push(BarItem::Entry(
                    BarEntry::new("编辑章节与元数据…", EditMetadata).enabled(self.file_editable()),
                ));
                items.push(BarItem::Entry(
                    BarEntry::new("生成代理文件…", MakeProxy)
                        .enabled(self.file_editable() && self.snapshot.has_video()),
                ));
                items.push(BarItem::Entry(
                    BarEntry::new("双机位并排导出…", MulticamExport)
                        .enabled(self.file_editable() && self.snapshot.has_video()),
                ));
                items.push(BarItem::Entry(
                    BarEntry::new("把列表拼成一个文件…", ConcatExport)
                        .enabled(self.playlist().file_count() >= 2),
                ));
                items.push(BarItem::Separator);
                items.push(BarItem::Entry(BarEntry::new("退出", Quit)));
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
                        .hint("N")
                        .enabled(navigable),
                ));
                items.push(BarItem::Entry(
                    BarEntry::new("下一集", Next).hint("P").enabled(navigable),
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
                    BarEntry::new("亮度直方图…", ShowScope).enabled(self.snapshot.has_video()),
                ));
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
                items.push(BarItem::Entry(
                    BarEntry::new("播放列表", TogglePlaylist)
                        .hint("L")
                        .checked(self.settings.show_playlist),
                ));
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
            BarAction::EditAudio => self.open_remux_dialog(EditTarget::Audio, cx),
            BarAction::EditSubtitle => self.open_remux_dialog(EditTarget::Subtitle, cx),
            BarAction::EditMetadata => self.open_metadata_dialog(window, cx),
            BarAction::MakeProxy => self.open_proxy_dialog(cx),
            BarAction::ShowScope => self.open_scope_dialog(cx),
            BarAction::MulticamExport => self.open_multicam_dialog(cx),
            BarAction::ConcatExport => self.open_concat_dialog(cx),
            BarAction::SetAsDefaultPlayer => self.set_default_player(),
            BarAction::OpenDefaultApps => self.open_default_apps(),
            BarAction::RemoveAssociations => self.remove_associations(window, cx),
            BarAction::ToggleMaintainAssociations => self.toggle_maintain_associations(),
            // 版本、引擎状态、配置位置这些信息一条提示条装不下，直接开模态窗口。
            BarAction::About => self.open_settings(Some(SettingsTab::About)),
            BarAction::Quit => {
                cx.quit();
                return;
            }
        }
        self.poke();
        cx.notify();
    }

    // -- 文件关联 ----------------------------------------------------------
}
