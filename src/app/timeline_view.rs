//! 工程模式的时间线：标题条、标尺、播放头与工程会话（新建 / 打开 / 关闭）。
//!
//! 轨道行、片段绘制、拖动换轨与轨道开关在 `timeline_tracks.rs`。缩放 / 滚动 /
//! 坐标换算的数字在 `crate::timeline::ViewState` 里，能用单测钉住；写盘与未保存
//! 确认在 `project_file.rs`。
//!
//! 布局固定为「上方预览 + 下方时间线」：进入工程模式后右侧面板与控制条都不画，
//! 画面区让出底部这一条（见 `render_video_area` 与 `controls_visible`）。

use super::*;
use crate::timeline::Id;

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
        // 预览播放属于工程：工程关掉了，时钟和 mpv 都不该继续跑。
        self.stop_timeline_playback(cx);
        if self.project.take().is_some() {
            // 内容已经存过或被用户主动丢弃，`.autosave` 再留着只会下次白问一句。
            self.clear_autosave();
            self.project_path = None;
            self.project_saved = None;
            self.history = None;
            self.timeline_dragging = false;
            self.timeline_menu = None;
            self.timeline_preview = None;
            self.timeline_loaded_at = None;
            self.flash("已关闭工程");
        }
        self.poke();
        cx.notify();
    }

    /// 新建一个空工程：默认 1080p30，自带一条视频轨与一条音频轨。
    pub(super) fn new_project(&mut self, cx: &mut Context<Self>) {
        // 换工程 = 换播放源：上一个工程的预览时钟和已加载的素材都作废。
        self.stop_timeline_playback(cx);
        // 普通播放里的上一部片子不属于这个空工程：连画面带播放一起停掉，
        // 否则它的画面会一直挂在预览区，还会随着 mpv 继续解码不停刷新。
        self.stop_and_clear_frame(cx);
        self.timeline_preview = None;
        self.timeline_loaded_at = None;
        // 上一个工程已经被保存或丢弃，它的 `.autosave` 不该跟着新工程活下来。
        self.clear_autosave();
        let project = Project::new();
        // 空工程还没落盘，但也没丢任何东西：把当前状态当基准，关掉时不必再问。
        self.project_saved = Some(project.clone());
        self.project_path = None;
        self.history = Some(UndoHistory::new(project.sequence.clone()));
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
                    this.request_project_action(AfterSaveConfirm::OpenProject(path), cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 真正读一份 `.ngyproj` 进内存。
    ///
    /// `recovery` 是用户同意恢复时读的那份自动保存。入口在
    /// [`PlayerApp::open_project_with_recovery`]：那边先问一句，这里只管装载。
    pub(super) fn load_project(
        &mut self,
        path: PathBuf,
        recovery: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let restored = recovery.is_some();
        let source = recovery.unwrap_or_else(|| path.clone());
        // 换工程 = 换播放源：上一个工程的预览时钟和已加载的素材都作废。
        self.stop_timeline_playback(cx);
        self.timeline_preview = None;
        self.timeline_loaded_at = None;
        // 上一个工程的菜单指向的片段在这个工程里不存在。
        self.timeline_menu = None;
        match Project::open(&source) {
            Ok(project) => {
                let duration = project.sequence.duration();
                let playhead = project.ui.playhead.secs();
                self.timeline = ViewState::new();
                match project.ui.zoom {
                    // 老工程没存过视图状态，按播放头居中打开。
                    Some(zoom) => {
                        self.timeline.set_zoom(
                            zoom,
                            ViewState::max_zoom(project.sequence.fps),
                            duration,
                        );
                        self.timeline.scroll_to(project.ui.scroll, duration);
                    }
                    None => self.timeline.center_on(playhead, duration),
                }
                self.timeline_dragging = false;
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "工程".to_string());
                // 从自动保存恢复的内容还没落进工程文件：基准取盘上那份，否则
                // 「未保存」标记会消失，用户以为已经存过了。盘上那份读不出来时
                // 基准留空，同样按「有未保存改动」对待。
                self.project_saved = if restored {
                    Project::open(&path).ok()
                } else {
                    Some(project.clone())
                };
                self.remember_project(&path);
                self.project_path = Some(path);
                self.history = Some(UndoHistory::new(project.sequence.clone()));
                self.project = Some(project);
                if restored {
                    // 恢复出来的改动也怕再崩一次：重新排上自动保存。
                    self.note_edit();
                    self.flash("已从自动保存恢复，记得保存工程");
                } else {
                    self.autosave.mark_saved();
                    self.flash(format!("已打开工程「{name}」"));
                }
                // 打开就出一帧：进来先看到黑屏，会让人以为工程打不开。
                self.timeline_preview = None;
                self.preview_at_playhead(cx);
            }
            Err(error) => self.flash(format!("打开工程失败：{error}")),
        }
        self.poke();
        cx.notify();
    }

    /// 把窗口横坐标换成时间线时间，并移动播放头。
    ///
    /// 不调用播放器的 seek：工程模式下时间线是预览，拖动播放头只改工程里的位置。
    /// `snap` 是按住 Shift：落点对齐到 0.01 秒网格，读数稳定可复现。
    pub(super) fn seek_timeline_to(&mut self, x: Pixels, snap: bool, cx: &mut Context<Self>) {
        // 拖播放头等于用户自己接管定位：正在放就停下，否则时钟会和手指抢播放头，
        // 松手之后播放头还在往前跑。
        self.stop_timeline_playback(cx);
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let lane_x = f32::from(x) - TIMELINE_GUTTER;
        let duration = project.sequence.duration();
        let seconds = seek_seconds(&self.timeline, lane_x, duration, snap);
        self.timeline_snap = snap;
        let mut at = Tick::from_secs(seconds);
        // 磁吸与 Shift 网格互斥：两套落点规则叠在一起会互相抢，按住 Shift 时
        // 用户要的就是精确读数，不该再被拉到标记上。
        if !snap {
            let targets: Vec<Tick> = project
                .ui
                .in_point
                .into_iter()
                .chain(project.ui.out_point)
                .chain(project.sequence.markers.iter().map(|marker| marker.at))
                .collect();
            at = snap_to_targets(&self.timeline, at, &targets);
        }
        if let Some(project) = self.project.as_mut() {
            project.ui.playhead = at;
        }
        // 播放头动了，画面跟着走（M4-T01）。
        self.preview_at_playhead(cx);
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

    /// 把编辑结果写回工程并记入撤销历史。所有改序列的入口都走这里。
    pub(super) fn commit_edit(&mut self, sequence: crate::timeline::Sequence) {
        if let Some(project) = self.project.as_mut() {
            project.sequence = sequence;
        }
        self.remember_history();
    }

    /// 把当前序列快照进撤销栈。轨道开关这类不经 `ops` 的改动，改完手动调一次。
    pub(super) fn remember_history(&mut self) {
        let Some(history) = self.history.as_mut() else {
            return;
        };
        if let Some(project) = self.project.as_ref() {
            history.push(project.sequence.clone());
        }
        // 编辑路径最后都收敛到这里，自动保存的 30 秒计时也就挂在这一处。
        self.note_edit();
    }

    /// 撤销一步。
    pub(super) fn undo_edit(&mut self, cx: &mut Context<Self>) {
        let previous = self
            .history
            .as_mut()
            .and_then(|history| history.undo().cloned());
        let Some(sequence) = previous else {
            self.flash("没有可撤销的操作");
            return;
        };
        self.apply_history(sequence);
        self.flash("已撤销");
        cx.notify();
    }

    /// 重做一步。
    pub(super) fn redo_edit(&mut self, cx: &mut Context<Self>) {
        let next = self
            .history
            .as_mut()
            .and_then(|history| history.redo().cloned());
        let Some(sequence) = next else {
            self.flash("没有可重做的操作");
            return;
        };
        self.apply_history(sequence);
        self.flash("已重做");
        cx.notify();
    }

    /// 撤销 / 重做共用：换回历史状态，并把指向已消失片段的选中项清掉。
    fn apply_history(&mut self, sequence: crate::timeline::Sequence) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        project.sequence = sequence;
        prune_selection(&mut project.ui.selected, &project.sequence);
        // 撤销 / 重做同样是「内容改了」，要重新计时。
        self.note_edit();
        self.poke();
    }

    /// 键盘缩放：锚点是播放头。比鼠标缩放粗一档，`-` / `=` 各按一次就是一级。
    pub(super) fn zoom_timeline_step(&mut self, zoom_in: bool, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let anchor = self.timeline.x_of(project.ui.playhead.secs());
        self.zoom_timeline(if zoom_in { 1.25 } else { 0.8 }, anchor);
        cx.notify();
    }

    /// 缩放到整条序列刚好可见，右侧留 5% 余量。
    pub(super) fn fit_timeline(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let duration = project.sequence.duration();
        let seconds = duration.secs().max(1.0) as f32;
        let zoom = self.timeline.width() / seconds * 0.95;
        let max_zoom = ViewState::max_zoom(project.sequence.fps);
        self.timeline.set_zoom(zoom, max_zoom, duration);
        self.timeline.scroll_to(0.0, duration);
        cx.notify();
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
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _window, cx| {
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
                if event.modifiers.secondary(cx) {
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
        root = root.child(self.render_timeline_header(cx));

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
                    this.seek_timeline_to(event.position.x, event.modifiers.shift, cx);
                    this.poke();
                    cx.notify();
                }),
            )
            // 右键不看播放头在哪儿：工作区的清除入口得有一个位置无关的兜底。
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.open_ruler_menu(event.position, cx);
                }),
            );

        let step = view.tick_step();
        let minor_step = step / 5.0;
        for tick in visible_ticks(view) {
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
        // 工作区：标尺下沿一条色带。标记竖线盖在它上面，两者互不遮挡。
        if let Some((start, end)) = project.ui.range() {
            let left = view.x_of(start.secs()).max(0.0);
            let right = view.x_of(end.secs()).min(lane_width);
            if right > left {
                ruler = ruler.child(
                    div()
                        .absolute()
                        .left(px(left))
                        .top(px(18.0))
                        .h(px(6.0))
                        .w(px(right - left))
                        .rounded(px(2.0))
                        .bg(gpui_kit::rgba(0x5aa9e6aa)),
                );
            }
        }

        // 工作区两端：只有一端时也必须画出来。色带要两端齐全才成立，只设入点时
        // 界面上原本什么变化都没有，用户既以为快捷键没生效，也没法把播放头拖回来。
        for (at, label) in [(project.ui.in_point, "入"), (project.ui.out_point, "出")] {
            let Some(at) = at else {
                continue;
            };
            let x = view.x_of(at.secs());
            if x < 0.0 || x > lane_width {
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
                        .bg(gpui_kit::rgba(0x5aa9e6cc)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px((x + 3.0).min((lane_width - 10.0).max(0.0))))
                        .top_0()
                        .text_size(px(9.0))
                        .text_color(gpui_kit::rgb(0x5aa9e6))
                        .child(label),
                );
        }

        // 标记：贯穿标尺的竖线加顶上一颗小方块。名字不上标尺——刻度文字已经够挤了。
        for marker in &sequence.markers {
            let x = view.x_of(marker.at.secs());
            if x < 0.0 || x > lane_width {
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
                        .bg(gpui_kit::rgba(0xf0c67499)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(x - 3.0))
                        .top_0()
                        .size(px(6.0))
                        .rounded(px(1.0))
                        .bg(gpui_kit::rgba(0xf0c674)),
                );
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

            // 拖动时把精确时间贴在播放头旁边：标题条在窗口右端，盯着标尺的眼睛
            // 看不到；只有整秒的读数也没法对准 8.00 秒。
            if self.timeline_dragging {
                let bubble_w = 112.0;
                let left = if playhead_x + 8.0 + bubble_w <= window_width {
                    playhead_x + 8.0
                } else {
                    (playhead_x - 8.0 - bubble_w).max(TIMELINE_GUTTER)
                };
                let label = if self.timeline_snap {
                    format!("{} ⇧0.01s", format_time_precise(playhead))
                } else {
                    format_time_precise(playhead)
                };
                root = root.child(
                    div()
                        .absolute()
                        .left(px(left))
                        .top(px(TIMELINE_HEADER + 3.0))
                        .w(px(bubble_w))
                        .h(px(18.0))
                        .px(px(6.0))
                        .flex()
                        .items_center()
                        .rounded(px(3.0))
                        .bg(if self.timeline_snap {
                            gpui_kit::rgba(0x5aa9ff33)
                        } else {
                            gpui_kit::rgba(0x1b1b24f2)
                        })
                        .text_size(px(10.0))
                        .text_color(ui::accent())
                        .child(label),
                );
            }
        }

        root.into_any_element()
    }
}

/// 撤销 / 重做之后，选中集合里指向已消失片段的 id 要清掉，
/// 否则下一次删除会拿一个不存在的 id 去查表并报错。
fn prune_selection(selected: &mut Vec<Id>, sequence: &crate::timeline::Sequence) {
    selected.retain(|id| sequence.clip(*id).is_some());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{AssetId, ClipItem, Rational, Sequence};

    #[test]
    fn 历史回退后清掉指向已删片段的选中() {
        let mut sequence = Sequence::new("测试", Rational::FPS_25, 1920, 1080);
        let track = sequence.tracks[0].id;
        let id = sequence.alloc_id();
        sequence.track_mut(track).unwrap().clips.push(ClipItem::new(
            id,
            AssetId::new(1),
            Tick::ZERO,
            Tick::from_secs(1.0),
            Tick::ZERO,
        ));

        let mut selected = vec![id, Id::new(999)];
        prune_selection(&mut selected, &sequence);
        assert_eq!(selected, vec![id], "只留下还在序列里的");
    }
}
