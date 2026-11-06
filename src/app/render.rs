//! `Render` 与 `Drop`：把各分区拼成整窗，并在退出时收尾。

use super::*;

impl Render for PlayerApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.swap_frame();
        self.drop_retired_frames(window);
        self.drop_retired_previews(window);
        self.sync_clip_source(cx);
        self.sync_metadata_source(cx);
        self.sync_proxy_source(cx);
        self.sync_scope_source(cx);
        self.sync_scenes_source(cx);
        self.sync_multicam_source(cx);
        self.poll_clip_preview(cx);
        self.poll_ab_loop(cx);
        self.sync_timeline_view(window);
        self.update_render_target(window);
        self.sync_on_top(window, cx);
        // 工程开关可能由拖放等菜单之外的路径触发，收掉另一套菜单的残留 open 状态。
        self.sync_menu_bar_mode();

        let title = if self.snapshot.media_title.is_empty() {
            "视频播放器".to_string()
        } else {
            format!("{} · 视频播放器", self.snapshot.media_title)
        };
        window.set_window_title(&title);

        let fullscreen = window.is_fullscreen();
        // 全屏是「只管放片」的形态：菜单栏完全不进场，右侧面板整条收起，屏幕上
        // 只剩画面和一条随鼠标出现、停一会儿又淡出的控制条。普通播放器全屏之后
        // 就是这个样子——窗口标题栏让位给画面，而不是把整个界面原样铺满屏幕。
        let bar_visible = !fullscreen;
        let video_top = if fullscreen {
            px(0.0)
        } else {
            px(MENUBAR_HEIGHT)
        };
        let osd_top = if fullscreen {
            px(28.0)
        } else {
            px(MENUBAR_HEIGHT + 16.0)
        };
        // 面板宽度只算一次：画面、控制条、面板本身都按同一个数让位。
        let panel_width = self.side_panel_width(window);
        // 工程模式下画面区让出底部这一条，时间线从窗口底边往上铺。
        let timeline_height = self.timeline_height(window);

        div()
            .id("root")
            .relative()
            .size_full()
            .bg(bg())
            .text_color(text())
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &TogglePlay, _window, cx| this.toggle_play(cx)))
            .on_action(
                cx.listener(|this, _: &SeekForward, _window, cx| this.seek_by(SEEK_STEP, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SeekBackward, _window, cx| this.seek_by(-SEEK_STEP, cx)),
            )
            .on_action(cx.listener(|this, _: &StepBackward, _window, cx| this.step_frame(-1, cx)))
            .on_action(cx.listener(|this, _: &StepForward, _window, cx| this.step_frame(1, cx)))
            .on_action(
                cx.listener(|this, _: &VolumeUp, _window, cx| this.change_volume(VOLUME_STEP, cx)),
            )
            .on_action(
                cx.listener(|this, _: &VolumeDown, _window, cx| {
                    this.change_volume(-VOLUME_STEP, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleMute, _window, cx| this.toggle_mute(cx)))
            .on_action(cx.listener(|this, _: &SubtitleEarlier, _window, cx| {
                this.set_subtitle_delay(-0.1, cx)
            }))
            .on_action(
                cx.listener(|this, _: &SubtitleLater, _window, cx| {
                    this.set_subtitle_delay(0.1, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleFullscreen, window, cx| {
                this.toggle_fullscreen(window);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleMini, window, cx| {
                this.toggle_mini(window, cx);
            }))
            .on_action(cx.listener(|this, _: &TogglePlaylist, window, cx| {
                this.toggle_playlist(window);
                this.poke();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenFile, window, cx| {
                this.pick_file(window, cx);
            }))
            // 没有工程时按 Ctrl+S 不该弹任何东西：播放器里没有被「保存」的对象。
            .on_action(cx.listener(|this, _: &SaveProject, window, cx| {
                if this.project.is_some() {
                    this.save_project(Some(window), cx, None);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SaveProjectAs, window, cx| {
                if this.project.is_some() {
                    this.save_project_as(Some(window), cx, None);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SplitPlayhead, _window, cx| {
                if this.project.is_some() {
                    this.split_at_playhead(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleMarker, _window, cx| {
                if this.project.is_some() {
                    this.toggle_marker_at_playhead(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SetInPoint, _window, cx| {
                if this.project.is_some() {
                    this.set_in_point(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SetOutPoint, _window, cx| {
                if this.project.is_some() {
                    this.set_out_point(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ZoomTimelineIn, _window, cx| {
                if this.project.is_some() {
                    this.zoom_timeline_step(true, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ZoomTimelineOut, _window, cx| {
                if this.project.is_some() {
                    this.zoom_timeline_step(false, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FitTimeline, _window, cx| {
                if this.project.is_some() {
                    this.fit_timeline(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &UndoEdit, _window, cx| {
                if this.project.is_some() {
                    this.undo_edit(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &RedoEdit, _window, cx| {
                if this.project.is_some() {
                    this.redo_edit(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NextFile, _window, cx| this.next_file(cx)))
            .on_action(cx.listener(|this, _: &PreviousFile, _window, cx| this.previous_file(cx)))
            .on_action(cx.listener(|this, _: &SpeedUp, _window, cx| {
                this.set_speed(this.snapshot.speed + 0.25, cx)
            }))
            .on_action(cx.listener(|this, _: &SpeedDown, _window, cx| {
                this.set_speed(this.snapshot.speed - 0.25, cx)
            }))
            .on_action(cx.listener(|this, _: &StopPlayback, _window, cx| {
                this.send(Command::Stop);
                cx.notify();
            }))
            // 帮助和画面设置现在都落在设置窗口里，只是打开时停的标签页不同。
            .on_action(cx.listener(|this, _: &ToggleHelp, _window, cx| {
                this.toggle_settings(SettingsTab::Shortcuts);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenPictureSettings, _window, cx| {
                this.toggle_settings(SettingsTab::Picture);
                cx.notify();
            }))
            .on_action(
                cx.listener(|this, _: &TakeScreenshot, _window, cx| this.take_screenshot(cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleStats, _window, cx| {
                this.show_stats ^= true;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &DeleteSelected, window, cx| {
                // 工程模式与播放器模式抢同一个键：时间线里删片段，列表里删文件。
                if this.project.is_some() {
                    // 全屏时时间线整条让给画面，选中看不见，不能盲删。
                    if !window.is_fullscreen() {
                        this.delete_selected_clips(cx);
                    }
                    return;
                }
                // 面板关着的时候选择是看不见的，这时候删等于盲删，直接忽略。
                // 全屏里面板同样不画（见本函数的渲染部分），一个道理。
                if this.settings.show_playlist && !window.is_fullscreen() {
                    this.remove_selection(cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &ExitOverlay, window, cx| this.exit_overlay(window, cx)),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                // 未保存确认压在最上层，先给它；往下依次是重命名输入框、只在
                // 输入框有焦点时接管键盘的裁切面板，以及各个模态对话框。
                // 不拦住的话，输入框里的字符会顺带触发播放器的快捷键。
                if this.on_project_confirm_key(event, cx)
                    || this.on_delete_track_confirm_key(event, cx)
                    || this.on_rename_key(event, cx)
                    || this.on_clip_key(event, cx)
                    || this.on_remux_key(event, cx)
                    || this.on_metadata_key(event, cx)
                    || this.on_proxy_key(event, cx)
                    || this.on_scope_key(event, cx)
                    || this.on_scenes_key(event, cx)
                    || this.on_multicam_key(event, cx)
                    || this.on_convert_key(event, cx)
                {
                    cx.stop_propagation();
                    return;
                }
                // `?` 不走键位表：GPUI 的键位串匹配的是**键帽字符**（`/` 而不是
                // `?`），而且不同键盘布局上 `?` 落在哪个键也不一样。直接看实际
                // 打出来的字符，`shift-/`、法语 AZERTY、德语 QWERTZ 都能命中。
                if event.keystroke.key_char.as_deref() == Some("?") {
                    this.toggle_settings(SettingsTab::Shortcuts);
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    // 点任意别处先收起右键菜单；菜单项自己的点击会截断冒泡。
                    let lost_focus = this.clear_clip_focus();
                    let closed = this.item_menu.take().is_some()
                        | this.tab_menu.take().is_some()
                        | this.timeline_menu.take().is_some();
                    if closed || lost_focus {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    // 空白处右键同样收起菜单（行上/页签上的右键会截断，转去弹新菜单）。
                    let closed = this.item_menu.take().is_some()
                        | this.tab_menu.take().is_some()
                        | this.timeline_menu.take().is_some();
                    if closed {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.poke();
                if this.dragging.is_some() {
                    this.update_progress_drag(event.position.x);
                }
                if this.volume_dragging {
                    this.set_volume_from_x(event.position.x);
                }
                if this.speed_dragging {
                    this.set_speed_from_x(event.position.x, cx);
                }
                if this.playlist_dragging {
                    this.drag_playlist_width(event.position.x, window);
                }
                if this.timeline_dragging {
                    this.seek_timeline_to(event.position.x, event.modifiers.shift, cx);
                }
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _window, cx| this.release_drags(cx)),
            )
            // 拖到窗口外再松手时命中测试是空的，窗口里一个 hitbox 都没有，
            // 上面那个 `on_mouse_up` 永远不会执行，拖动状态就卡住了。
            // `on_mouse_up_out` 只在抬起时指针不在本元素范围内触发，正好兜住这一路。
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _window, cx| this.release_drags(cx)),
            )
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _window, cx| {
                let delta = match event.delta {
                    ScrollDelta::Pixels(delta) => f32::from(delta.y),
                    ScrollDelta::Lines(delta) => delta.y * 24.0,
                };
                if delta.abs() < 0.01 {
                    return;
                }
                // Ctrl/⌘ + 滚轮逐帧查看。滚轮按「事件」算，一格走一帧；触控板给的是
                // 像素增量，攒够一格才走，否则一次轻滑就是几十帧。
                //
                // 方向：上滚回到前面的帧、下滚往后的帧。GPUI 里上滚是正 y，
                // 与「加音量」同向，所以步进要取反。
                if event.modifiers.secondary(cx) {
                    let steps = match event.delta {
                        ScrollDelta::Lines(delta) => -(delta.y.signum() as i32),
                        ScrollDelta::Pixels(delta) => {
                            let accumulated = this.frame_scroll - f32::from(delta.y);
                            let steps = (accumulated / FRAME_SCROLL_PX).trunc();
                            // 只保留不足一格的余量，换方向时不会先欠着上一次的账。
                            this.frame_scroll = accumulated - steps * FRAME_SCROLL_PX;
                            steps as i32
                        }
                    };
                    if steps != 0 {
                        this.step_frame(steps, cx);
                    }
                    return;
                }
                this.frame_scroll = 0.0;
                this.change_volume(
                    if delta > 0.0 {
                        VOLUME_STEP
                    } else {
                        -VOLUME_STEP
                    },
                    cx,
                );
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _window, cx| {
                let dropped: Vec<PathBuf> = paths.0.iter().cloned().collect();
                this.open_paths(dropped, cx);
                this.poke();
                cx.notify();
            }))
            .child(self.render_video_area(video_top, panel_width, timeline_height, cx))
            // 转换 / 裁切 / 播放列表互斥，占同一条右侧通道；前两者开着时优先它们。
            //
            // 全屏时这条通道整条不画：全屏要看的是画面本身，不是它旁边那一栏。
            // 面板的开关状态原样留着，退出全屏就回来——切一下全屏不该把正在跑的
            // 裁切/转换任务取消掉。
            // 工程模式下这三块都不画，面板状态原样留着。
            .when(!fullscreen && self.project.is_none(), |this| {
                this.children(self.render_convert_panel(video_top, panel_width, cx))
                    .children(self.render_clip_panel(video_top, panel_width, cx))
                    .when(
                        self.settings.show_playlist
                            && self.clip.is_none()
                            && self.convert.is_none()
                            && !self.show_history,
                        |this| this.child(self.render_playlist(video_top, panel_width, cx)),
                    )
            })
            // 工程模式的右侧：编辑历史贴着预览区，下边界让开时间线。
            .when(!fullscreen && self.project.is_some(), |this| {
                this.children(self.render_history_panel(
                    video_top,
                    panel_width,
                    timeline_height,
                    cx,
                ))
            })
            .when(self.controls_visible(), |this| {
                this.child(self.render_controls(panel_width, cx))
            })
            .when(timeline_height > 0.0, |this| {
                this.child(self.render_timeline(window, timeline_height, cx))
            })
            .children(self.render_menu(panel_width, cx))
            .children(self.render_osd(osd_top, panel_width))
            .children(self.render_screenshot_card(osd_top, cx))
            .children(self.render_stats_card(osd_top, cx))
            .when(bar_visible, |this| {
                this.child(self.render_menu_bar(window, cx))
            })
            .children(self.render_error(panel_width, cx))
            // 右键菜单在普通浮层之上、模态面板之下。
            .children(self.render_item_menu(cx))
            .children(self.render_tab_menu(cx))
            .children(self.render_timeline_menu(cx))
            .children(self.render_settings(window, cx))
            .children(self.render_remux_dialog(cx))
            .children(self.render_metadata_dialog(cx))
            .children(self.render_proxy_dialog(cx))
            .children(self.render_scope_dialog(cx))
            .children(self.render_scenes_dialog(cx))
            .children(self.render_multicam_dialog(cx))
            .children(self.render_concat_dialog(cx))
            .children(self.render_playlist_import_dialog(cx))
            .children(self.render_file_info(cx))
            .children(self.render_delete_track_confirm(cx))
            // 重命名是模态的，压在所有东西上面。
            .children(self.render_tab_rename(cx))
            // 未保存确认比它还靠上一层：它一旦出现，答案决定的是整份工程的去留。
            .children(self.render_project_confirm(cx))
    }
}

impl Drop for PlayerApp {
    fn drop(&mut self) {
        // 窗口关闭时必须让 mpv 停下来，否则音频线程会继续跑到进程退出。
        if let Some(engine) = &self.engine {
            engine.shutdown();
        }
        // 正常退出不必留崩溃恢复：那份 `.autosave` 只会在下次打开时白问一句。
        //
        // 但「点窗口右上角 ×」这条路我们拦不住（gpui-base 0.6.6 没有 should_close，
        // 只有事后的 on_window_closed），它同样会走到这里。所以只在**没有未保存改动**
        // 时才删：有改动说明用户是带着没保存的东西被关掉的，留着下次打开能捡回来。
        // 用户明确选过「不保存」的路径已经由 `request_project_action` 清掉了。
        if !self.project_modified() {
            self.clear_autosave();
        }
        self.settings.save();
        // 列表本来就随改动落过盘，这里再存一次是兜住「拖入后一次都没点过」的情形。
        self.playlists.save();
    }
}
