//! 工程模式的时间线：标题条、标尺、播放头与工程会话（新建 / 打开 / 关闭）。
//!
//! 轨道行、片段绘制、拖动换轨与轨道开关在 `timeline_tracks.rs`。缩放 / 滚动 /
//! 坐标换算的数字在 `crate::timeline::ViewState` 里，能用单测钉住；写盘与未保存
//! 确认在 `project_file.rs`。
//!
//! 布局固定为「上方预览 + 下方时间线」：进入工程模式后右侧面板与控制条都不画，
//! 画面区让出底部这一条（见 `render_video_area` 与 `controls_visible`）。

use super::*;

impl PlayerApp {
    /// 时间线占多高。窗口高时按比例，窄窗口不低于 170，宽屏不超过 320。
    pub(super) fn timeline_height(&self, window: &Window) -> f32 {
        if self.project.is_none() || window.is_fullscreen() {
            return 0.0;
        }
        let available = f32::from(window.viewport_size().height) - MENUBAR_HEIGHT;
        // 窗口很矮时优先保证预览还剩一块地方，时间线可以缩到 120。
        let ceiling = (available - 120.0).max(120.0);
        (available * 0.34)
            .clamp(TIMELINE_MIN_HEIGHT, TIMELINE_MAX_HEIGHT)
            .min(ceiling)
    }

    /// 按窗口宽度同步视口宽度。渲染与鼠标换算共用同一个「一屏多少秒」。
    pub(super) fn sync_timeline_view(&mut self, window: &Window) {
        let Some(duration) = self.project.as_ref().map(|p| p.sequence.duration()) else {
            return;
        };
        let width = (f32::from(window.viewport_size().width) - TIMELINE_GUTTER).max(1.0);
        self.timeline.set_width(width, duration);
    }

    /// 退出工程模式。未保存改动由调用方先确认（见 `request_project_action`）。
    pub(super) fn close_project(&mut self, cx: &mut Context<Self>) {
        if self.project.take().is_some() {
            self.project_path = None;
            self.project_saved = None;
            self.timeline_dragging = false;
            self.flash("已关闭工程");
        }
        self.poke();
        cx.notify();
    }

    /// 新建一个空工程：默认 1080p30，自带一条视频轨与一条音频轨。
    pub(super) fn new_project(&mut self, cx: &mut Context<Self>) {
        let project = Project::new();
        // 空工程还没落盘，但也没丢任何东西：把当前状态当基准，关掉时不必再问。
        self.project_saved = Some(project.clone());
        self.project_path = None;
        self.project = Some(project);
        self.timeline = ViewState::new();
        self.timeline_dragging = false;
        self.flash("已新建工程");
        self.poke();
        cx.notify();
    }

    /// 选一个 `.ngyproj` 打开。对话框阻塞在后台线程里，结果走 oneshot 回来。
    pub(super) fn pick_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let mut picker = rfd::FileDialog::new()
            .add_filter("工程文件", formats::PROJECT)
            .add_filter("所有文件", &["*"])
            .set_parent(window);
        if let Some(dir) = &self.settings.last_dir {
            picker = picker.set_directory(dir);
        }
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(picker.pick_file());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if let Some(path) = picked {
                    this.remember_dir(&path);
                    // 当前工程可能还有没保存的东西，先过一遍确认。
                    this.request_project_action(AfterSaveConfirm::OpenProject(path), None, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 打开一个 `.ngyproj`。拖入、命令行、菜单都走这里。
    pub(super) fn open_project_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match Project::open(&path) {
            Ok(project) => {
                let duration = project.sequence.duration();
                let playhead = project.ui.playhead.secs();
                self.timeline = ViewState::new();
                self.timeline.center_on(playhead, duration);
                self.timeline_dragging = false;
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "工程".to_string());
                self.project_saved = Some(project.clone());
                self.project_path = Some(path);
                self.project = Some(project);
                self.flash(format!("已打开工程「{name}」"));
            }
            Err(error) => self.flash(format!("打开工程失败：{error}")),
        }
        self.poke();
        cx.notify();
    }

    /// 把窗口横坐标换成时间线时间，并移动播放头。
    ///
    /// 不调用播放器的 seek：工程模式下时间线是预览，拖动播放头只改工程里的位置。
    pub(super) fn seek_timeline_to(&mut self, x: Pixels) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let lane_x = f32::from(x) - TIMELINE_GUTTER;
        let seconds = self.timeline.time_at(lane_x);
        let seconds = seconds.clamp(0.0, project.sequence.duration().secs().max(0.0));
        if let Some(project) = self.project.as_mut() {
            project.ui.playhead = Tick::from_secs(seconds);
        }
    }

    pub(super) fn scroll_timeline(&mut self, pixels: f32) {
        let Some(duration) = self.project.as_ref().map(|p| p.sequence.duration()) else {
            return;
        };
        self.timeline.scroll_by(pixels, duration);
    }

    /// 以 `anchor_x`（轨道区内的横坐标）为锚点缩放。
    pub(super) fn zoom_timeline(&mut self, factor: f32, anchor_x: f32) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let max_zoom = ViewState::max_zoom(project.sequence.fps);
        let duration = project.sequence.duration();
        self.timeline
            .zoom_around(factor, anchor_x.max(0.0), max_zoom, duration);
    }

    // -- 渲染 --------------------------------------------------------------

    pub(super) fn render_timeline(
        &self,
        window: &Window,
        height: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(project) = &self.project else {
            return div().into_any_element();
        };
        let sequence = &project.sequence;
        let duration = sequence.duration();
        let view = &self.timeline;
        let window_width = f32::from(window.viewport_size().width);
        let lane_width = (window_width - TIMELINE_GUTTER).max(1.0);
        let playhead = project.ui.playhead.secs();

        let mut root = div()
            .id("timeline")
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(height))
            .overflow_hidden()
            .bg(surface())
            .border_t_1()
            .border_color(gpui_kit::rgb(0x26262e))
            // 面板上的点击不该漏到画面去（画面那一层负责播放/暂停）。
            .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                cx.stop_propagation();
            }))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                if this.project.is_none() {
                    return;
                }
                let (dx, dy) = match event.delta {
                    ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y)),
                    ScrollDelta::Lines(delta) => (delta.x * 24.0, delta.y * 24.0),
                };
                if dx.abs() < 0.01 && dy.abs() < 0.01 {
                    return;
                }
                cx.stop_propagation();
                if event.modifiers.secondary() {
                    // 上滚放大、下滚缩小；锚点是鼠标所在的轨道区横坐标。
                    let factor = if dy > 0.0 { 1.25 } else { 0.8 };
                    let anchor = f32::from(event.position.x) - TIMELINE_GUTTER;
                    this.zoom_timeline(factor, anchor);
                } else {
                    // 触控板横向滑动优先用 x；普通滚轮只有 y，也拿它横着滚。
                    let pixels = if dx.abs() > 0.01 { -dx } else { -dy };
                    this.scroll_timeline(pixels);
                }
                cx.notify();
            }));

        // 标题条：序列名、帧率、总时长、播放头时间与当前缩放。
        root = root.child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(TIMELINE_HEADER))
                .flex()
                .items_center()
                .justify_between()
                .px(px(10.0))
                .border_b_1()
                .border_color(gpui_kit::rgb(0x26262e))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .text_size(px(13.0))
                        .text_color(text())
                        .child(sequence.name.clone())
                        // 未保存标记：整个工程只有这一处提示，别让它太安静。
                        .when(self.project_modified(), |this| {
                            this.child(div().text_color(ui::accent()).child("*"))
                        })
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(text_faint())
                                .child(format!("{} fps", sequence.fps)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.0))
                        .text_size(px(11.0))
                        .text_color(text_muted())
                        .child(format_time(duration.secs()))
                        .child(format_time(playhead))
                        .child(format!("{:.1} 像素/秒", view.zoom())),
                ),
        );

        // 标尺：主刻度带文字，之间四分之一的短刻度。
        let mut ruler = div()
            .absolute()
            .top(px(TIMELINE_HEADER))
            .left(px(TIMELINE_GUTTER))
            .right_0()
            .h(px(TIMELINE_RULER))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.timeline_dragging = true;
                    this.seek_timeline_to(event.position.x);
                    this.poke();
                    cx.notify();
                }),
            );

        let step = view.tick_step();
        let minor_step = step / 5.0;
        for tick in visible_ticks(view, duration) {
            let x = view.x_of(tick);
            if x < -1.0 || x > lane_width {
                continue;
            }
            ruler = ruler
                .child(
                    div()
                        .absolute()
                        .left(px(x))
                        .top_0()
                        .bottom_0()
                        .w(px(1.0))
                        .bg(gpui_kit::rgb(0x3a3a44)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(x + 4.0))
                        .top(px(3.0))
                        .text_size(px(10.0))
                        .text_color(text_faint())
                        .child(format_time(tick)),
                );
            for minor in 1..5 {
                let at = tick + minor_step * minor as f64;
                let minor_x = view.x_of(at);
                if minor_x < 0.0 || minor_x > lane_width {
                    continue;
                }
                ruler = ruler.child(
                    div()
                        .absolute()
                        .left(px(minor_x))
                        .bottom_0()
                        .h(px(5.0))
                        .w(px(1.0))
                        .bg(gpui_kit::rgb(0x2c2c34)),
                );
            }
        }
        root = root.child(ruler);

        // 轨道与左侧名称列。只画视口内、面板高度内的轨道，三小时工程也一样快。
        let tracks_top = TIMELINE_HEADER + TIMELINE_RULER;
        let visible_tracks = ((height - tracks_top) / (TIMELINE_TRACK_HEIGHT + TIMELINE_TRACK_GAP))
            .floor()
            .max(0.0) as usize;
        for (index, track) in sequence
            .tracks
            .iter()
            .take(visible_tracks.max(1))
            .enumerate()
        {
            let top = tracks_top + index as f32 * (TIMELINE_TRACK_HEIGHT + TIMELINE_TRACK_GAP);
            let lane = self.render_timeline_lane(project, track, top, lane_width, cx);
            root = root
                .child(lane)
                .child(self.render_timeline_label(track, top, cx));
        }

        if sequence.tracks.is_empty() {
            root = root.child(
                div()
                    .absolute()
                    .top(px(tracks_top))
                    .left_0()
                    .right_0()
                    .h(px(TIMELINE_TRACK_HEIGHT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.0))
                    .text_color(text_faint())
                    .child("这条序列还没有轨道"),
            );
        }

        // 播放头：贯穿标尺与轨道区。滚出视口就不画，免得贴在边上装成「在 0 秒」。
        let playhead_x = TIMELINE_GUTTER + view.x_of(playhead);
        if playhead_x >= TIMELINE_GUTTER && playhead_x <= window_width {
            root = root
                .child(
                    div()
                        .absolute()
                        .left(px(playhead_x - 1.0))
                        .top(px(TIMELINE_HEADER))
                        .bottom_0()
                        .w(px(2.0))
                        .bg(ui::accent()),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(playhead_x - 4.0))
                        .top(px(TIMELINE_HEADER - 3.0))
                        .size(px(9.0))
                        .rounded_full()
                        .bg(ui::accent()),
                );
        }

        root.into_any_element()
    }
}
