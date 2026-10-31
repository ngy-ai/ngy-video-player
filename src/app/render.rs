//! `Render` 与 `Drop`：把各分区拼成整窗，并在退出时收尾。

use super::*;

impl Render for PlayerApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.swap_frame(window);
        self.drop_retired_previews(window);
        self.sync_clip_source(cx);
        self.poll_clip_preview(cx);
        self.update_render_target(window);
        self.sync_on_top(window, cx);

        let title = if self.snapshot.media_title.is_empty() {
            "视频播放器".to_string()
        } else {
            format!("{} · 视频播放器", self.snapshot.media_title)
        };
        window.set_window_title(&title);

        let fullscreen = window.is_fullscreen();
        // 全屏时把菜单栏让给鼠标：和底栏一起出现、一起消失，避免画面永久缩水。
        let bar_visible = !fullscreen || self.controls_visible();
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
            .on_action(cx.listener(|this, _: &ToggleFullscreen, window, cx| {
                this.toggle_fullscreen(window);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleMini, window, cx| {
                this.toggle_mini(window, cx);
            }))
            .on_action(cx.listener(|this, _: &TogglePlaylist, _window, cx| {
                this.set_playlist_visible(!this.settings.show_playlist);
                this.poke();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenFile, window, cx| {
                this.pick_file(window, cx);
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
            .on_action(cx.listener(|this, _: &DeleteSelected, _window, cx| {
                // 面板关着的时候选择是看不见的，这时候删等于盲删，直接忽略。
                if this.settings.show_playlist {
                    this.remove_selection(cx);
                }
            }))
            .on_action(
                cx.listener(|this, _: &ExitOverlay, window, cx| this.exit_overlay(window, cx)),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                // 重命名输入框在最上层；裁切面板只在输入框有焦点时接管键盘；
                // 轨道编辑对话框是模态的。不拦住的话，输入框里的字符会顺带
                // 触发播放器的快捷键。
                if this.on_rename_key(event, cx)
                    || this.on_clip_key(event, cx)
                    || this.on_remux_key(event, cx)
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
                    let closed = this.item_menu.take().is_some() | this.tab_menu.take().is_some();
                    if closed || lost_focus {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                    // 空白处右键同样收起菜单（行上/页签上的右键会截断，转去弹新菜单）。
                    let closed = this.item_menu.take().is_some() | this.tab_menu.take().is_some();
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
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
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
                if event.modifiers.secondary() {
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
            .child(self.render_video_area(video_top, panel_width, cx))
            // 转换 / 裁切 / 播放列表互斥，占同一条右侧通道；前两者开着时优先它们。
            .children(self.render_convert_panel(video_top, panel_width, cx))
            .children(self.render_clip_panel(video_top, panel_width, cx))
            .when(
                self.settings.show_playlist && self.clip.is_none() && self.convert.is_none(),
                |this| this.child(self.render_playlist(video_top, panel_width, cx)),
            )
            .when(self.controls_visible(), |this| {
                this.child(self.render_controls(panel_width, cx))
            })
            .children(self.render_menu(panel_width, cx))
            .children(self.render_osd(osd_top, panel_width))
            .children(self.render_screenshot_card(osd_top, cx))
            .when(bar_visible, |this| {
                this.child(self.render_menu_bar(window, cx))
            })
            .children(self.render_error(panel_width, cx))
            // 右键菜单在普通浮层之上、模态面板之下。
            .children(self.render_item_menu(cx))
            .children(self.render_tab_menu(cx))
            .children(self.render_settings(window, cx))
            .children(self.render_remux_dialog(cx))
            .children(self.render_file_info(cx))
            // 重命名是模态的，压在所有东西上面。
            .children(self.render_tab_rename(cx))
    }
}

impl Drop for PlayerApp {
    fn drop(&mut self) {
        // 窗口关闭时必须让 mpv 停下来，否则音频线程会继续跑到进程退出。
        if let Some(engine) = &self.engine {
            engine.shutdown();
        }
        self.settings.save();
        // 列表本来就随改动落过盘，这里再存一次是兜住「拖入后一次都没点过」的情形。
        self.playlists.save();
    }
}
