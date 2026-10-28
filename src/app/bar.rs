//! 菜单栏数据：条目构造与动作分发。

use super::*;

impl PlayerApp {
    pub(super) fn bar_entries(&self, menu: BarMenu, window: &Window) -> Vec<BarItem> {
        use BarAction::*;

        let mut items: Vec<BarItem> = Vec::new();
        match menu {
            BarMenu::File => {
                items.push(BarItem::Entry(
                    BarEntry::new("打开文件…", OpenFile).hint("O"),
                ));
                items.push(BarItem::Entry(BarEntry::new("打开文件夹…", OpenFolder)));
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
                items.push(BarItem::Entry(
                    BarEntry::new("截图", TakeScreenshot)
                        .hint("S")
                        .enabled(self.snapshot.has_video()),
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
                items.push(BarItem::Entry(BarEntry::new(
                    "从文件加载字幕…",
                    LoadSubtitle,
                )));
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
                items.push(BarItem::Entry(BarEntry::new(
                    "关于 ngy-video-player",
                    About,
                )));
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
            BarAction::TogglePlaylist => {
                self.set_playlist_visible(!self.settings.show_playlist);
            }
            BarAction::ToggleFullscreen => self.toggle_fullscreen(window),
            BarAction::ToggleMini => self.toggle_mini(window, cx),
            BarAction::ToggleHelp => self.toggle_settings(SettingsTab::Shortcuts),
            BarAction::OpenSettings(tab) => self.open_settings(tab),
            BarAction::TakeScreenshot => self.take_screenshot(cx),
            BarAction::SetAsDefaultPlayer => self.set_default_player(),
            BarAction::OpenDefaultApps => self.open_default_apps(),
            BarAction::RemoveAssociations => self.remove_associations(window, cx),
            BarAction::ToggleMaintainAssociations => self.toggle_maintain_associations(),
            BarAction::About => {
                self.flash(format!(
                    "ngy-video-player {}\nGPUI + libmpv",
                    env!("CARGO_PKG_VERSION")
                ));
            }
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
