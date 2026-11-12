//! 一条轨道的片段区：画片段、拖动换轨、两种落点预览。
//!
//! 两种拖拽共用这一层：拖已有片段（`TimelineDrag`）与从素材库拖素材（`AssetDrag`）。
//! GPUI 按拖拽值的类型分派，所以同一条轨道上挂了两套 `on_drag_move` / `on_drop`，
//! 各自只收到自己那种。

use super::super::library::{AssetDrag, lane_offset};
use super::*;
use gpui_kit::DragMoveEvent;

impl PlayerApp {
    /// 一条轨道上的片段。位置按视口换算，滚动时只画看得见的那几段。
    ///
    /// 片段可以拖到别的同类型轨道上；落点由 `on_drag_move` 算好放进
    /// `self.timeline_drop`，松手时 `on_drop` 才真正改序列。
    pub(crate) fn render_timeline_lane(
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
            .id(format!("timeline-lane-{}", track_id.raw()))
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
                        // 修饰键也在这里记一笔：松手时读不到键盘状态。
                        drag.mods.set((
                            event.event.modifiers.alt,
                            event.event.modifiers.shift,
                            event.event.modifiers.control,
                        ));
                        (drag.clip, drag.grab.get())
                    };
                    let Some(project) = this.project.as_ref() else {
                        return;
                    };
                    let fps = project.sequence().fps;
                    let lane_x = f32::from(position.x) - f32::from(event.bounds.origin.x);
                    let seconds = this.timeline.time_at(lane_x - grab).max(0.0);
                    let start = snap_to_frame(Tick::from_secs(seconds), fps);
                    this.set_timeline_drop(
                        TimelineDrop {
                            clip,
                            to_track: track_id,
                            start,
                            mode: if event.event.modifiers.shift {
                                Placement::Insert
                            } else {
                                Placement::Overwrite
                            },
                        },
                        cx,
                    );
                },
            ))
            .on_drop(cx.listener(move |this, drag: &TimelineDrag, _window, cx| {
                this.drop_timeline_clip(drag, cx);
            }))
            // 素材库往轨道上拖。与片段拖动是两种拖拽值，GPUI 按类型分开派发，
            // 各自只收到自己那种。
            .on_drag_move(cx.listener(
                move |this, event: &DragMoveEvent<AssetDrag>, _window, cx| {
                    let position = event.event.position;
                    if !event.bounds.contains(&position) {
                        return;
                    }
                    let lane_x =
                        lane_offset(f32::from(position.x), f32::from(event.bounds.origin.x));
                    let drag = event.drag(cx).clone();
                    // 按住 Shift 是「插入」（后面整体后移），不按是「覆盖」。
                    this.set_asset_drop(track_id, lane_x, &drag, event.event.modifiers.shift, cx);
                },
            ))
            .on_drop(cx.listener(move |this, drag: &AssetDrag, _window, cx| {
                this.drop_assets(track_id, drag, cx);
            }))
            // 拖关键帧标记：落点按「片段内相对时间」算，所以要先知道标记属于哪个片段。
            .on_drag_move(cx.listener(
                move |this, event: &DragMoveEvent<KeyframeDrag>, _window, cx| {
                    let position = event.event.position;
                    if !event.bounds.contains(&position) {
                        return;
                    }
                    let (clip, from) = {
                        let drag = event.drag(cx);
                        (drag.clip, drag.from)
                    };
                    let lane_x = f32::from(position.x) - f32::from(event.bounds.origin.x);
                    this.set_keyframe_drop(track_id, clip, from, lane_x, cx);
                },
            ))
            .on_drop(cx.listener(move |this, drag: &KeyframeDrag, _window, cx| {
                this.drop_keyframe(drag, cx);
            }))
            // 拖两端修剪抓手：落点按被拖的那一端算，不需要抓取偏移。
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<TrimDrag>, _window, cx| {
                    let position = event.event.position;
                    if !event.bounds.contains(&position) {
                        return;
                    }
                    let (clip, side, grab) = {
                        let drag = event.drag(cx);
                        (drag.clip, drag.side, drag.grab.get())
                    };
                    let Some(project) = this.project.as_ref() else {
                        return;
                    };
                    let fps = project.sequence().fps;
                    let lane_x = f32::from(position.x) - f32::from(event.bounds.origin.x);
                    let seconds = this.timeline.time_at(lane_x - grab).max(0.0);
                    this.trim_drop = Some(TrimDrop {
                        clip,
                        side,
                        at: snap_to_frame(Tick::from_secs(seconds), fps),
                        alt: event.event.modifiers.alt,
                        shift: event.event.modifiers.shift,
                    });
                    cx.notify();
                }),
            )
            .on_drop(cx.listener(move |this, drag: &TrimDrag, _window, cx| {
                this.drop_trim(drag, cx);
            }))
            // 点轨道空白处取消选中；点片段时片段自己截断了事件。
            .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.clear_selection(cx);
            }));

        // 字幕轨画的是文字条目：它没有素材、倍率与效果，也不参与拖放换轨，
        // 所以单独一条绘制路径，别和素材片段的拖动 / 转场标记搅在一起。
        if track.kind == crate::timeline::TrackKind::Subtitle {
            for entry in track
                .visible_subtitles(Tick::from_secs(view.scroll()), Tick::from_secs(visible_end))
            {
                let left = view.x_of(entry.start.secs()).max(0.0);
                let right = view.x_of(entry.end.secs()).min(lane_width);
                let width = (right - left).max(2.0);
                let label: SharedString = entry.label().into();
                let id = entry.id;
                lane = lane.child(
                    div()
                        .id(format!("timeline-subtitle-{}", id.raw()))
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
                        .opacity(if track.muted { 0.4 } else { 1.0 })
                        .when(width > 44.0, |this| {
                            this.text_size(px(11.0))
                                .text_color(gpui_kit::rgba(0xffffffee))
                                .whitespace_nowrap()
                                .child(label)
                        })
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            cx.stop_propagation();
                            this.open_subtitle_dialog(None, Some(id), Tick::ZERO, window, cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                                cx.stop_propagation();
                                this.open_subtitle_item_menu(id, event.position, cx);
                            }),
                        ),
                );
            }
            // 空白处右键新建：起点就用播放头（标尺上先点好位置，面板里还能再改）。
            // 用播放头而不是鼠标坐标，免得 DPI 缩放下的坐标换算把字幕放到别处。
            return lane
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        let start = this
                            .project
                            .as_ref()
                            .map(|project| project.ui.playhead)
                            .unwrap_or(Tick::ZERO);
                        this.open_subtitle_dialog(Some(track_id), None, start, window, cx);
                    }),
                )
                .into_any_element();
        }

        // 只取视口里的片段：万级片段的工程每帧从头扫一遍，滚动就会掉帧。
        for clip in
            track.visible_clips(Tick::from_secs(view.scroll()), Tick::from_secs(visible_end))
        {
            let start = clip.start.secs();
            let end = clip.end().secs();
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
            let selected = project.ui.selected.contains(&clip_id);
            let ghost_name: SharedString = name.clone().into();
            let grab = Rc::new(Cell::new(0.0f32));
            let drag_grab = grab.clone();
            let mods = Rc::new(Cell::new((false, false, false)));

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
                    // 选中用白描边。透明边框也占位，选中与否不影响片段尺寸。
                    .border_1()
                    .border_color(if selected {
                        gpui_kit::rgba(0xffffffff)
                    } else {
                        gpui_kit::rgba(0x00000000)
                    })
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
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        // 片段叠在轨道上，不截断的话点片段也会被当成点空白。
                        cx.stop_propagation();
                        this.select_clip(clip_id, cx);
                    }))
                    // 右键先选中再弹菜单：菜单上的每一项都作用于这一段。
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                            cx.stop_propagation();
                            this.open_timeline_menu(clip_id, track_id, event.position, cx);
                        }),
                    )
                    // 拖到别的同类型轨道；落回同一条轨道就是改起点。
                    .on_drag(
                        TimelineDrag {
                            clip: clip_id,
                            grab,
                            mods,
                        },
                        move |_drag, offset, _window, cx| {
                            drag_grab.set(f32::from(offset.x));
                            cx.new(|_| TimelineGhost {
                                name: ghost_name.clone(),
                            })
                        },
                    ),
            );

            // 修剪抓手：两端各 6 像素。拖左端改入点、右端改出点，松手才落一步撤销。
            // 修饰键的语义见 `drop_trim`：Alt 波纹修剪、Shift 拖右端是滚动编辑。
            if !track.locked {
                for side in [TrimSide::Start, TrimSide::End] {
                    let side_grab = Rc::new(Cell::new(0.0f32));
                    let write_grab = side_grab.clone();
                    let x = match side {
                        TrimSide::Start => left,
                        TrimSide::End => (right - 6.0).max(left),
                    };
                    let id = format!(
                        "trim-{}-{}",
                        match side {
                            TrimSide::Start => "start",
                            TrimSide::End => "end",
                        },
                        clip_id.raw()
                    );
                    lane = lane.child(
                        div()
                            .id(id)
                            .absolute()
                            .left(px(x))
                            .top(px(4.0))
                            .bottom(px(4.0))
                            .w(px(6.0))
                            // 一直存在、只在悬停时才有颜色：看不见的抓手抓不住。
                            .bg(gpui_kit::rgba(0xffffff1e))
                            .on_drag(
                                TrimDrag {
                                    clip: clip_id,
                                    side,
                                    grab: side_grab,
                                },
                                move |_drag, offset, _window, cx| {
                                    write_grab.set(f32::from(offset.x));
                                    cx.new(|_| TimelineGhost {
                                        name: "修剪".into(),
                                    })
                                },
                            ),
                    );
                }
            }

            // 关键帧标记：片段内相对时间换成序列时间，画成贴着片段下沿的小竖条。
            // 每帧重算一遍，只扫视口内的片段，几万个点的工程也不会卡。
            //
            // 外面那层是 11 像素宽的抓手：3 像素的竖条好看但抓不住，而拖动阈值只有
            // 2 像素。抓手是透明的，画出来的仍是那根竖条。
            for mark in clip.keys.marks() {
                let x = view.x_of((clip.start + mark).secs());
                if x < 0.0 || x > lane_width {
                    continue;
                }
                let ghost: SharedString = format!("{:.2}s", mark.secs()).into();
                lane = lane.child(
                    div()
                        .id(format!("timeline-key-{}-{}", clip_id.raw(), mark.micros()))
                        .absolute()
                        .left(px(x - 5.5))
                        .bottom(px(2.0))
                        .w(px(11.0))
                        .h(px(12.0))
                        .flex()
                        .items_end()
                        .justify_center()
                        .cursor(CursorStyle::ResizeLeftRight)
                        .opacity(if track.muted { 0.4 } else { 1.0 })
                        .child(
                            div()
                                .w(px(3.0))
                                .h(px(9.0))
                                .rounded(px(1.5))
                                .bg(gpui_kit::rgba(0xffffffff))
                                .border_1()
                                .border_color(gpui_kit::rgba(0x00000066)),
                        )
                        .on_drag(
                            KeyframeDrag {
                                clip: clip_id,
                                from: mark,
                            },
                            move |_drag, _offset, _window, cx| {
                                cx.new(|_| TimelineGhost {
                                    name: ghost.clone(),
                                })
                            },
                        ),
                );
            }
        }

        // 接缝上的转场标记：宽按转场时长算、跨在接缝两侧，点一下打开转场面板。
        // 画在片段之后，所以压在片段上面；缩得很粗时至少留 6 像素，否则点不到。
        for transition in &track.transitions {
            let Some(after) = track.clip(transition.after) else {
                continue;
            };
            let center = view.x_of(after.start.secs());
            let half = (view.x_of(after.start.secs() + transition.duration.secs()) - center) / 2.0;
            let width = (half * 2.0).max(6.0);
            let left = center - width / 2.0;
            if left > lane_width || left + width < 0.0 {
                continue;
            }
            let after_id = after.id;
            lane = lane.child(
                div()
                    .id(format!("timeline-transition-{}", transition.id.raw()))
                    .absolute()
                    .left(px(left))
                    .top(px(4.0))
                    .bottom(px(4.0))
                    .w(px(width))
                    .rounded(px(3.0))
                    .bg(gpui_kit::rgba(0x00d8ff55))
                    .border_1()
                    .border_color(gpui_kit::rgba(0x00d8ffdd))
                    .opacity(if track.muted { 0.4 } else { 1.0 })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                            // 标记压在片段上：不截断的话点它会顺带选中 / 拖走片段。
                            cx.stop_propagation();
                            this.open_transition_menu(after_id, event.position, cx);
                        }),
                    ),
            );
        }

        // 拖关键帧时画一条竖线。`has_active_drag` 是必须的：拖到别的轨道上松手时
        // `on_drop` 不会触发，只看 `keyframe_drop` 会让竖线留在原地。
        if let Some(drop) = self.keyframe_drop
            && cx.has_active_drag()
            && track.clip(drop.clip).is_some()
            && let Some(clip) = project.sequence().clip(drop.clip)
        {
            let x = view.x_of((clip.start + drop.to).secs());
            lane = lane.child(
                div()
                    .absolute()
                    .left(px(x - 0.75))
                    .top(px(2.0))
                    .bottom(px(2.0))
                    .w(px(1.5))
                    .bg(ui::accent())
                    .opacity(0.9),
            );
        }

        // 拖动预览：贴着落点画一个半透明框，松手之前就能看出会落到哪。
        if let Some(drop) = self.timeline_drop
            && drop.to_track == track_id
            && let Some(clip) = project.sequence().clip(drop.clip)
        {
            let left = view.x_of(drop.start.secs()).max(0.0);
            let right = view
                .x_of((drop.start + clip.duration()).secs())
                .min(lane_width);
            let width = (right - left).max(2.0);
            let label = match drop.mode {
                Placement::Overwrite => "覆盖",
                Placement::Insert => "插入",
            };
            lane = lane.child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(4.0))
                    .bottom(px(4.0))
                    .w(px(width))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(ui::accent())
                    .bg(gpui_kit::rgba(0x3f6fd855))
                    .overflow_hidden()
                    .px(px(5.0))
                    .flex()
                    .items_center()
                    .text_size(px(10.0))
                    .text_color(text())
                    .whitespace_nowrap()
                    // 框太窄时写不下字，宁可空着也不让文字外溢。
                    .when(width > 44.0, |this| this.child(label)),
            );
        }

        // 素材拖进来的落点：同样画一个框，放不下时换成拒绝色——
        // 松手之前就知道「这条轨道不收」，不用等提示。
        // `has_active_drag` 是必须的：拖出轨道再松手时 `on_drop` 不会触发，
        // 只看 `asset_drop` 会让框一直留在轨道上。
        if let Some(drop) = self.asset_drop
            && drop.track == track_id
            && cx.has_active_drag()
        {
            let left = view.x_of(drop.start.secs()).max(0.0);
            let right = view
                .x_of((drop.start + drop.duration).secs())
                .min(lane_width);
            let width = (right - left).max(2.0);
            let (border, fill, ink) = if drop.ok {
                (ui::accent(), gpui_kit::rgba(0x3f6fd855), text())
            } else {
                (ui::danger(), gpui_kit::rgba(0xd03a3a44), ui::danger())
            };
            lane = lane.child(
                div()
                    .absolute()
                    .left(px(left))
                    .top(px(4.0))
                    .bottom(px(4.0))
                    .w(px(width))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(border)
                    .bg(fill)
                    .overflow_hidden()
                    .px(px(5.0))
                    .flex()
                    .items_center()
                    .text_size(px(10.0))
                    .text_color(ink)
                    .whitespace_nowrap()
                    // 框里写清「覆盖 / 插入」与将放多长；太窄时宁可空着。
                    .when(width > 72.0, |this| {
                        let mode = match drop.mode {
                            Placement::Overwrite => "覆盖",
                            Placement::Insert => "插入",
                        };
                        this.child(format!("{mode} {}", format_time(drop.duration.secs())))
                    }),
            );
        }

        lane.into_any_element()
    }
}
