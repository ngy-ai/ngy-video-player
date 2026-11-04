//! 工程模式的时间线：标尺、轨道、片段与播放头。
//!
//! 只负责「把 `Project` 画出来」、鼠标换算与拖动换轨；缩放 / 滚动 / 坐标换算的数字在
//! `crate::timeline::ViewState` 里，能用单测钉住。真正改序列的操作交给
//! `crate::timeline::ops`，这里只负责算出落点；写盘与未保存确认在 `project_file.rs`。
//!
//! 布局固定为「上方预览 + 下方时间线」：进入工程模式后右侧面板与控制条都不画，
//! 画面区让出底部这一条（见 `render_video_area` 与 `controls_visible`）。

use super::*;
use crate::timeline::{EditOp, Id, Rational, apply as apply_edit};
use gpui_kit::DragMoveEvent;

/// 拖动中的片段。GPUI 的拖放按值的类型分发，专用类型不会和「从资源管理器拖文件进来」混在一起。
struct TimelineDrag {
    clip: Id,
    /// 抓取点在片段内部的横向像素偏移。落点按片段左边缘算，不按鼠标位置算，
    /// 否则抓住长片段中间拖动时，片段会整个跳到鼠标下面。
    grab: Rc<Cell<f32>>,
}

/// 跟着鼠标走的拖动预览：一颗只显示名字的胶囊。
struct TimelineGhost {
    name: SharedString,
}

impl Render for TimelineGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(180.0))
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(4.0))
            .bg(gpui_kit::rgba(0x232330f2))
            .border_1()
            .border_color(ui::accent())
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_size(px(11.0))
            .text_color(text())
            .child(self.name.clone())
    }
}

/// 拖动片段时算出来的落点。渲染预览与松手后执行都用它。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TimelineDrop {
    clip: Id,
    to_track: Id,
    start: Tick,
}

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

    /// 一条轨道上的片段。位置按视口换算，滚动时只画看得见的那几段。
    ///
    /// 片段可以拖到别的同类型轨道上；落点由 `on_drag_move` 算好放进
    /// `self.timeline_drop`，松手时 `on_drop` 才真正改序列。
    fn render_timeline_lane(
        &self,
        project: &Project,
        track: &crate::timeline::Track,
        top: f32,
        lane_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view = &self.timeline;
        let visible_end = view.scroll() + view.visible_seconds();
        let track_id = track.id;
        let mut lane = div()
            .absolute()
            .left(px(TIMELINE_GUTTER))
            .right_0()
            .top(px(top))
            .h(px(TIMELINE_TRACK_HEIGHT))
            .overflow_hidden()
            .border_b_1()
            .border_color(gpui_kit::rgb(0x22222a))
            .on_drag_move(cx.listener(
                move |this, event: &DragMoveEvent<TimelineDrag>, _window, cx| {
                    let position = event.event.position;
                    if !event.bounds.contains(&position) {
                        return;
                    }
                    // 抓取偏移只在 `on_drag` 的构造器里写得到，这里读出来还原片段左边缘。
                    let (clip, grab) = {
                        let drag = event.drag(cx);
                        (drag.clip, drag.grab.get())
                    };
                    let Some(project) = this.project.as_ref() else {
                        return;
                    };
                    let fps = project.sequence.fps;
                    let lane_x = f32::from(position.x) - f32::from(event.bounds.origin.x);
                    let seconds = this.timeline.time_at(lane_x - grab).max(0.0);
                    let start = snap_to_frame(Tick::from_secs(seconds), fps);
                    this.set_timeline_drop(
                        TimelineDrop {
                            clip,
                            to_track: track_id,
                            start,
                        },
                        cx,
                    );
                },
            ))
            .on_drop(cx.listener(move |this, drag: &TimelineDrag, _window, cx| {
                this.drop_timeline_clip(drag.clip, cx);
            }));

        for clip in &track.clips {
            let start = clip.start.secs();
            let end = clip.end().secs();
            if end <= view.scroll() || start >= visible_end {
                continue;
            }
            let left = view.x_of(start).max(0.0);
            let right = view.x_of(end).min(lane_width);
            let width = (right - left).max(2.0);
            let name = if clip.name.is_empty() {
                project
                    .asset(clip.asset_id)
                    .map(|asset| asset.display_name())
                    .unwrap_or_else(|| "片段".to_string())
            } else {
                clip.name.clone()
            };
            let clip_id = clip.id;
            let ghost_name: SharedString = name.clone().into();
            let grab = Rc::new(Cell::new(0.0f32));
            let drag_grab = grab.clone();

            lane = lane.child(
                div()
                    // `on_drag` 挂在有身份的元素上：命中测试要能认出「拖的是哪一个」。
                    .id(format!("timeline-clip-{}", clip_id.raw()))
                    .absolute()
                    .left(px(left))
                    .top(px(4.0))
                    .bottom(px(4.0))
                    .w(px(width))
                    .rounded(px(4.0))
                    .overflow_hidden()
                    .px(px(5.0))
                    .flex()
                    .items_center()
                    .bg(clip_color(track.kind))
                    // 关掉的轨道整体压暗：不改颜色本身，缩放很小的时候也能看出它不参与输出。
                    .opacity(if track.muted { 0.4 } else { 1.0 })
                    .when(width > 44.0, |this| {
                        this.text_size(px(11.0))
                            .text_color(gpui_kit::rgba(0xffffffee))
                            .whitespace_nowrap()
                            .child(name)
                    })
                    // 拖到别的同类型轨道；落回同一条轨道就是改起点。
                    .on_drag(
                        TimelineDrag {
                            clip: clip_id,
                            grab,
                        },
                        move |_drag, offset, _window, cx| {
                            drag_grab.set(f32::from(offset.x));
                            cx.new(|_| TimelineGhost {
                                name: ghost_name.clone(),
                            })
                        },
                    ),
            );
        }

        // 拖动预览：贴着落点画一个半透明框，松手之前就能看出会落到哪。
        if let Some(drop) = self.timeline_drop
            && drop.to_track == track_id
            && let Some(clip) = project.sequence.clip(drop.clip)
        {
            let left = view.x_of(drop.start.secs()).max(0.0);
            let right = view
                .x_of((drop.start + clip.duration()).secs())
                .min(lane_width);
            lane = lane.child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(4.0))
                    .bottom(px(4.0))
                    .w(px((right - left).max(2.0)))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(ui::accent())
                    .bg(gpui_kit::rgba(0x3f6fd855)),
            );
        }

        lane.into_any_element()
    }

    /// 左侧名称列：轨道类型、名字与两个状态开关。
    fn render_timeline_label(
        &self,
        track: &crate::timeline::Track,
        top: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let track_id = track.id;

        // 视频 / 字幕轨的「关掉」是隐藏，音频轨是静音：同一个状态，两种说法。
        let (mute_icon, mute_tip) = match track.kind {
            crate::timeline::TrackKind::Audio => (
                if track.muted {
                    IconName::VolumeX
                } else {
                    IconName::Volume2
                },
                if track.muted {
                    "取消静音"
                } else {
                    "静音这一轨"
                },
            ),
            _ => (
                if track.muted {
                    IconName::EyeOff
                } else {
                    IconName::Eye
                },
                if track.muted {
                    "显示这一轨"
                } else {
                    "隐藏这一轨"
                },
            ),
        };
        let (lock_icon, lock_tip) = if track.locked {
            (IconName::Lock, "解锁这一轨")
        } else {
            (IconName::LockOpen, "锁定这一轨（禁止编辑）")
        };

        div()
            .absolute()
            .left_0()
            .top(px(top))
            .w(px(TIMELINE_GUTTER))
            .h(px(TIMELINE_TRACK_HEIGHT))
            .flex()
            .flex_col()
            .justify_center()
            .gap(px(2.0))
            .px(px(8.0))
            .border_b_1()
            .border_color(gpui_kit::rgb(0x22222a))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .size(px(7.0))
                            .rounded_full()
                            .bg(clip_color(track.kind)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(11.0))
                            // 关掉的轨道用更淡的颜色，一眼能从名字列看出它不参与输出。
                            .text_color(if track.muted {
                                text_faint()
                            } else {
                                text_muted()
                            })
                            .child(track.name.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .child(ui::icon_toggle_small(
                        format!("track-mute-{}", track_id.raw()),
                        mute_icon,
                        track.muted,
                        mute_tip,
                        cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.toggle_track_muted(track_id, cx);
                        }),
                    ))
                    .child(ui::icon_toggle_small(
                        format!("track-lock-{}", track_id.raw()),
                        lock_icon,
                        track.locked,
                        lock_tip,
                        cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.toggle_track_locked(track_id, cx);
                        }),
                    )),
            )
            .into_any_element()
    }

    /// 切换轨道的静音 / 隐藏。锁定不影响它——锁的是编辑，不是输出。
    pub(super) fn toggle_track_muted(
        &mut self,
        track: crate::timeline::Id,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let Some(track) = project.sequence.track_mut(track) else {
            return;
        };
        track.muted = !track.muted;
        let muted = track.muted;
        let kind = track.kind;
        let name = track.name.clone();

        let action = match (kind, muted) {
            (crate::timeline::TrackKind::Audio, true) => "静音",
            (crate::timeline::TrackKind::Audio, false) => "恢复声音",
            (_, true) => "隐藏",
            (_, false) => "显示",
        };
        self.flash(format!("轨道「{name}」已{action}"));
        cx.notify();
    }

    /// 切换轨道锁定。锁定的轨道拒绝一切编辑（校验在 `timeline::ops`）。
    pub(super) fn toggle_track_locked(
        &mut self,
        track: crate::timeline::Id,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let Some(track) = project.sequence.track_mut(track) else {
            return;
        };
        track.locked = !track.locked;
        let locked = track.locked;
        let name = track.name.clone();

        self.flash(format!(
            "轨道「{name}」已{}",
            if locked { "锁定" } else { "解锁" }
        ));
        cx.notify();
    }

    /// 记下落点。相同落点直接返回，拖动时指针每移动一个像素都重绘整窗太浪费。
    fn set_timeline_drop(&mut self, drop: TimelineDrop, cx: &mut Context<Self>) {
        if self.timeline_drop == Some(drop) {
            return;
        }
        self.timeline_drop = Some(drop);
        cx.notify();
    }

    /// 松手：把落点交给 `timeline::ops`，重叠 / 锁定 / 轨道类型这些校验都在那边。
    fn drop_timeline_clip(&mut self, clip: Id, cx: &mut Context<Self>) {
        let Some(drop) = self.timeline_drop.take() else {
            return;
        };
        if drop.clip != clip || self.project.is_none() {
            cx.notify();
            return;
        }

        let op = EditOp::Move {
            clip,
            to_track: drop.to_track,
            start: drop.start,
        };
        let outcome = match self.project.as_ref() {
            Some(project) => apply_edit(&project.sequence, &op),
            None => return,
        };
        match outcome {
            Ok(sequence) => {
                if let Some(project) = self.project.as_mut() {
                    project.sequence = sequence;
                }
                self.flash("已移动片段");
            }
            Err(error) => self.flash(format!("移动失败：{error}")),
        }
        self.poke();
        cx.notify();
    }
}

/// 片段底色：视频蓝、音频绿、字幕橙。整块颜色比图标更容易在缩放很小时认出来。
fn clip_color(kind: crate::timeline::TrackKind) -> gpui_kit::Rgba {
    match kind {
        crate::timeline::TrackKind::Video => gpui_kit::rgba(0x3f6fd8e6),
        crate::timeline::TrackKind::Audio => gpui_kit::rgba(0x2f8f5be6),
        crate::timeline::TrackKind::Subtitle => gpui_kit::rgba(0xb07a2ee6),
    }
}

/// 落点对齐到帧：像素换算出来的时间是连续值，不对齐会得到「半帧」的起点，
/// 导出时那一帧要么重复要么丢掉。
fn snap_to_frame(tick: Tick, fps: Rational) -> Tick {
    Tick::from_frames(tick.round_frames(fps), fps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 落点吸附到最近的帧() {
        let fps = Rational::FPS_25;
        assert_eq!(
            snap_to_frame(Tick::from_millis(10), fps),
            Tick::ZERO,
            "0.01 秒贴着第 0 帧"
        );
        assert_eq!(
            snap_to_frame(Tick::from_millis(30), fps),
            Tick::from_millis(40),
            "0.03 秒更靠近第 1 帧"
        );
        assert_eq!(
            snap_to_frame(Tick::from_millis(49), fps),
            Tick::from_millis(40)
        );
    }
}
