//! 视频区渲染：帧交换、横纵比、进度条与悬停预览。

use super::subtitle_guides;
use super::*;

/// 没有帧时是否该显示空状态提示。
///
/// 纯音频正在播放时既不会有画面，也不该一直挂着「正在加载…」：视频区保持黑屏。
/// 工程模式同理：预览由时间线喂帧，空序列本来就该是黑屏，「拖入视频文件」
/// 是播放器模式的引导，不该盖在预览区上。
fn show_placeholder(
    frame: &Option<Arc<RenderImage>>,
    snapshot: &Snapshot,
    in_project: bool,
) -> bool {
    !in_project && frame.is_none() && !snapshot.playing_without_video()
}

/// 这一帧是否属于当前文件、且当前文件确实该有画面。
///
/// 纯音频整代次不收帧：mpv 在 loadfile 期间会把上一部片子的最后一帧按
/// **新代次**重新渲染出来，光比对代次挡不住它。
pub(super) fn frame_accepted(epoch: u64, current: u64, audio_only: bool) -> bool {
    epoch == current && !audio_only
}

impl PlayerApp {
    /// 换片 / 停止：正在显示的和还在排队的帧都属于上一部片子。
    pub(super) fn discard_frame(&mut self) {
        self.incoming = None;
        if let Some(previous) = self.frame.take() {
            self.retired.push_back(previous);
        }
    }

    /// 发 `Command::Load` 之前调用：本地先作废旧画面，并挡住通道里在途的旧帧。
    /// 引擎随后下发的 `FrameCleared` 会把代次换成真正的新值。
    ///
    /// 纯音频从这一刻起就整代次不收帧：不等 mpv 上报轨道表，那段滞后正是
    /// 「切到第一个音频还留着上一个视频画面」的窗口。
    pub(super) fn begin_load(&mut self, path: &std::path::Path) {
        self.discard_frame();
        self.frame_epoch = FRAME_EPOCH_PENDING;
        self.loading_audio_only = formats::is_audio(path);
    }

    /// 停止播放并把画面清成黑屏（新建空工程时调用）。
    ///
    /// `Command::Stop` 要等引擎处理完才下发 `FrameCleared`，这期间 mpv 还在送旧帧；
    /// 本地先把代次推到「等待新代次」并丢掉已显示 / 排队中的帧，旧画面才不会
    /// 在这几十毫秒里又被贴回来。小窗共用同一路帧，也要同步清掉。
    pub(super) fn stop_and_clear_frame(&mut self, cx: &mut Context<Self>) {
        self.discard_frame();
        self.frame_epoch = FRAME_EPOCH_PENDING;
        self.loading_audio_only = false;
        if let Some(mini) = &self.mini {
            mini.update(cx, |mini, cx| {
                mini.discard_frame();
                cx.notify();
            });
        }
        self.clear_preview();
        self.send(Command::Stop);
        cx.notify();
    }

    pub(super) fn swap_frame(&mut self) {
        let Some(incoming) = self.incoming.take() else {
            return;
        };
        if let Some(previous) = self.frame.replace(incoming) {
            self.retired.push_back(previous);
        }
    }

    /// 把不再显示的帧还给 atlas。
    ///
    /// `drop_image` 只应该在没有元素再画它的时候调，所以放在渲染路径上，
    /// 并留两帧余量（刚换下来的那张可能还在 GPU 队列里被采样）。
    /// 没有新帧送来时也要跑一遍，否则换片清掉的旧帧会一直占着 atlas。
    pub(super) fn drop_retired_frames(&mut self, window: &mut Window) {
        while self.retired.len() > 2 {
            if let Some(stale) = self.retired.pop_front() {
                let _ = window.drop_image(stale);
            }
        }
    }

    /// 把不再显示的预览图还给 atlas。
    ///
    /// `drop_image` 只应该在没有元素再画它的时候调，所以在渲染路径上做，
    /// 而且留一张余量——刚换下来的那张可能还在 GPU 队列里被采样。
    pub(super) fn drop_retired_previews(&mut self, window: &mut Window) {
        while self.preview_retired.len() > 1 {
            if let Some(stale) = self.preview_retired.pop_front() {
                let _ = window.drop_image(stale);
            }
        }
    }

    /// 告诉取帧线程应该按多大尺寸、什么角度渲染。
    ///
    /// 传的是「画面在视频区域里该占的那一块」的物理像素，而不是整个区域：
    /// 选了 4:3 时这块就是 4:3，mpv 在里面按视频比例排好，界面再把它等比贴进
    /// 区域、四周补黑边。铺满那两档直接把区域本身当画布，变形或裁剪都交给 mpv。
    ///
    /// 区域比视频原始分辨率大时按原始分辨率渲染：放大不增加细节，只是让每帧
    /// 多拷几 MB。另外还要过一道渲染模式的高度上限（见 `RenderMode::height_cap`）。
    pub(super) fn update_render_target(&self, window: &mut Window) {
        let Some(engine) = &self.engine else {
            return;
        };
        let viewport = window.viewport_size();
        let scale = window.scale_factor();
        // 两侧面板占掉的那两条不算视频区域，否则缩画面时会被拉伸。
        let panel = self.side_panel_width(window);
        let library = self.library_width(window);
        let area_width = (f32::from(viewport.width) - panel - library).max(2.0) * scale;
        // 窗口模式下菜单栏占掉顶部一条、时间线占掉底部一条，渲染目标都要扣掉，
        // 否则画面会被裁掉一截或者在时间线后面白渲染。
        let chrome = if window.is_fullscreen() {
            0.0
        } else {
            MENUBAR_HEIGHT + self.timeline_height(window)
        };
        let area_height = (f32::from(viewport.height) - chrome) * scale;
        if area_width < 2.0 || area_height < 2.0 {
            return;
        }

        let area_aspect = area_width / area_height;
        let aspect = self.picture_aspect(area_aspect);
        // 在区域里按目标比例取最大的框；比例一致时就是整个区域。
        let (canvas_width, canvas_height) = if area_aspect > aspect {
            (area_height * aspect, area_height)
        } else {
            (area_width, area_width / aspect)
        };

        // 画布是「旋转之前」的朝向，所以这里直接和片源的宽高比，不用换轴。
        let native_width = self.snapshot.video_width as f32;
        let native_height = self.snapshot.video_height as f32;
        let fit = if native_width > 0.0 && native_height > 0.0 {
            (native_width / canvas_width)
                .min(native_height / canvas_height)
                .min(1.0)
        } else {
            1.0
        };

        let mut width = (canvas_width * fit).round().max(2.0) as u32;
        let mut height = (canvas_height * fit).round().max(2.0) as u32;
        // 性能模式封顶渲染高度：全屏看 4K 时，缩放和每帧回读那几个 MB 的像素
        // 是耗电的大头，而 720 行在正常观看距离下几乎看不出与全分辨率的差别。
        if let Some(cap) = self.settings.render_mode.height_cap()
            && height > cap
        {
            width = ((width as f32 * cap as f32 / height as f32).round() as u32).max(2);
            height = cap;
        }
        engine.set_render_target(width, height, self.settings.video_rotate.degrees());
        // 合成预览用同一块画布：窗口一变，合成器跟着换尺寸重新叠。
        if let Some(compositor) = &self.compositor {
            compositor.set_target(width, height);
        }
    }

    /// 画面该按什么比例排。
    ///
    /// 注意这里说的是**旋转之前**的画布：4:3 转 90 度之后在屏幕上是 3:4，
    /// 那正是「把 4:3 的画面竖起来看」应有的样子。而铺满那两档的诉求是
    /// 「占满窗口」，所以要按转过之后的结果倒推画布比例。
    pub(super) fn picture_aspect(&self, area_aspect: f32) -> f32 {
        let swapped = self.settings.video_rotate.swaps_axes();
        let aspect = match self.settings.video_aspect {
            VideoAspect::Stretch | VideoAspect::Crop => {
                if swapped {
                    area_aspect.recip()
                } else {
                    area_aspect
                }
            }
            mode => mode.ratio().unwrap_or_else(|| self.snapshot.aspect()),
        };
        // 极端值（比如片源报了个错误的宽高比）会让画布退化成一条线，夹一下。
        aspect.clamp(0.1, 10.0)
    }

    pub(super) fn progress_value(&self) -> f64 {
        self.dragging.unwrap_or(self.snapshot.position)
    }

    // -- 渲染 --------------------------------------------------------------

    pub(super) fn render_video_area(
        &self,
        top: Pixels,
        panel: f32,
        library: f32,
        bottom: f32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // 工程模式的预览靠时间线喂帧，空序列保持黑屏，不套播放器模式的引导语。
        let placeholder = show_placeholder(&self.frame, &self.snapshot, self.project.is_some());
        // 单路预览演不出两幅画面叠在一起：空档整块压黑，转场窗口里淡到黑再淡回来
        // （形状与导出侧的过渡一致）。播放器模式没有时间线，永远是 0；
        // 试听原片时画面与播放头无关，也不能拿时间线的空档去压黑它。
        let dim = if self.audition.is_some() {
            0.0
        } else {
            self.project
                .as_ref()
                .map(|project| {
                    crate::timeline::preview::overlay_dim(project.sequence(), project.ui.playhead)
                })
                .unwrap_or(0.0)
        };
        // 预览的字幕是一层界面文字：单路 mpv 画面上没有 libass 排版，字体与描边
        // 只有导出时才有。试听原片时画面与播放头无关，也不能拿时间线字幕盖上去。
        let subtitle = if self.audition.is_some() {
            None
        } else {
            self.project.as_ref().and_then(|project| {
                crate::timeline::subtitle_at(project.sequence(), project.ui.playhead)
            })
        };
        // 逐字 / 卡拉 OK：播放头落在第几个字，前面的字就换高亮色。libass 那种逐字
        // 渐变填充在界面叠层里做不了，只能按字分色；导出仍然走 ASS 的 `\kf`。
        let playhead = self
            .project
            .as_ref()
            .map(|project| project.ui.playhead)
            .unwrap_or(crate::timeline::Tick::ZERO);
        let sung = subtitle.and_then(|item| item.karaoke_index(playhead));
        div()
            .id("video-area")
            .absolute()
            .inset_0()
            .top(top)
            // 侧边栏是真正占地方的：画面让出那一条，而不是被盖住。
            .when(panel > 0.0, |this| this.right(px(panel)))
            .when(library > 0.0, |this| this.left(px(library)))
            // 工程模式下时间线占住窗口底部，画面在剩下的区域里居中。
            .when(bottom > 0.0, |this| this.bottom(px(bottom)))
            .bg(bg())
            .child(ui::video_surface(self.frame.clone()))
            // 黑场 / 转场压黑。`rgba` 的字节序是 RRGGBBAA，RGB 全 0 就是纯黑。
            .when(dim > 0.0, |this| {
                let alpha = (dim.clamp(0.0, 1.0) * 255.0).round() as u8;
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(gpui_kit::rgba(u32::from(alpha))),
                )
            })
            // 参考线压在字幕文字下面：文字该压得住线，而不是被线穿过。
            .when(
                self.project.is_some() && self.settings.show_safe_area,
                |this| this.child(subtitle_guides::safe_area_layer()),
            )
            // 字幕压在黑场之上：空档里也应该看得见字幕。字号按预览窗口折一档，
            // ASS 里的数字是按序列高度（1080p / 4K）定义的像素。
            .when_some(subtitle, |this, item| {
                let style = &item.style;
                let size = (style.size as f32 * 0.55).clamp(14.0, 64.0);
                let ink: Hsla = gpui_kit::rgb(style.color).into();
                let mut layer = div().absolute().inset_0().flex().flex_col().items_center();
                layer = match style.position.vertical() {
                    "start" => layer.justify_start().pt(px(24.0)),
                    "center" => layer.justify_center(),
                    _ => layer.justify_end().pb(px(24.0)),
                };
                let bubble = div()
                    .px(px(10.0))
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .bg(gpui_kit::rgba(0x00000066))
                    .text_size(px(size));
                let bubble = match (item.karaoke_enabled(), sung) {
                    (true, Some(index)) => {
                        let (done, rest) = item.karaoke_split(index);
                        bubble
                            .flex()
                            .items_center()
                            .child(div().text_color(gpui_kit::rgba(0xffd5_4aee)).child(done))
                            .child(div().text_color(ink).child(rest))
                    }
                    _ => bubble.text_color(ink).child(item.label()),
                };
                this.child(layer.child(bubble))
            })
            // 空状态只看「有没有帧」，不看 mpv 上报的宽高：width/height 属性在
            // 视频解码前是 0，用它会在一段正常播放的画面上盖一层「正在加载」。
            // 纯音频例外：它永远不会有帧，也不该一直挂着占位文字，黑屏即可。
            .when(placeholder, |this| {
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(14.0))
                        .child(icon(IconName::Film, 56.0, text_faint()))
                        .child(div().text_size(px(15.0)).text_color(text_muted()).child(
                            if self.playlist().is_empty() {
                                "拖入视频文件，或按 O 打开"
                            } else {
                                "正在加载…"
                            },
                        ))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(text_faint())
                                .child(format!(
                                    "支持 {} 种常见容器与编码 · 其余格式交给 libmpv 自动识别",
                                    formats::supported_extension_count()
                                )),
                        ),
                )
            })
            .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                // 双击全屏；单击切换播放。播放器里这是最不容易误触的默认组合。
                if event.click_count() >= 2 {
                    this.toggle_fullscreen(window);
                } else if this.menu.is_some() || this.menu_bar.is_some() {
                    this.menu = None;
                    this.menu_bar = None;
                } else {
                    this.toggle_play(cx);
                }
                cx.notify();
            }))
    }

    pub(super) fn render_progress(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let duration = self.snapshot.duration.max(0.001);
        let position = self.progress_value().clamp(0.0, duration);
        let ratio = (position / duration).clamp(0.0, 1.0) as f32;
        let dragging = self.dragging.is_some();
        let slot = self.progress_bounds.clone();
        // 预览浮层按进度条宽度定位；这里先取一次，后面 canvas 再更新也不影响这一帧。
        let slot_width = self.progress_bounds.get().size.width;

        let bar = div()
            .relative()
            .h(px(if dragging { 6.0 } else { 4.0 }))
            .w_full()
            .rounded(px(3.0))
            .bg(gpui_kit::rgba(0xffffff30))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(gpui_kit::relative(ratio))
                    .rounded(px(3.0))
                    .bg(ui::accent())
                    .child(
                        div()
                            .absolute()
                            .right(px(-4.5))
                            .top(px(-3.5))
                            .size(px(if dragging { 13.0 } else { 11.0 }))
                            .rounded_full()
                            .bg(ui::accent()),
                    ),
            );

        div()
            .id("progress")
            .relative()
            .w_full()
            .h(px(18.0))
            .flex()
            .items_center()
            .cursor(CursorStyle::PointingHand)
            .child(
                canvas(
                    move |bounds, _window, _cx| slot.set(bounds),
                    |_bounds, (), _window, _cx| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(bar)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.begin_progress_drag(event.position.x);
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _window, cx| {
                    this.end_progress_drag(cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                this.update_hover(Some(event.position.x), cx);
            }))
            // 指针离开这一条只能靠 `on_hover` 得知：移出去之后这棵子树
            // 再也收不到鼠标事件，光靠 on_mouse_move 浮层会一直挂在那里。
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if !*hovered {
                    this.update_hover(None, cx);
                }
            }))
            .children(
                self.hover
                    .as_ref()
                    .map(|hover| self.render_preview(hover, f32::from(slot_width))),
            )
    }

    /// 进度条上方的悬停预览：一张小图 + 对应时间。
    ///
    /// 浮层是进度条的绝对定位子节点，画在画面之上、控制条之外，
    /// 不占控制条的布局空间，也不会把进度条顶高。
    pub(super) fn render_preview(&self, hover: &HoverPreview, bar_width: f32) -> AnyElement {
        const WIDTH: f32 = 208.0;
        const PADDING: f32 = 3.0;
        const LABEL: f32 = 18.0;

        // 跟着指针走，但不许越出进度条两端——浮层跑到控制条外面看着像画错了。
        let left = (hover.ratio * bar_width - WIDTH * 0.5).clamp(0.0, (bar_width - WIDTH).max(0.0));

        // 图框高度按视频比例走：宽银幕片子不该被塞进一个 16:9 的框里。
        let aspect = self.snapshot.aspect().clamp(0.5, 4.0);
        let image_height = ((WIDTH - PADDING * 2.0) / aspect).clamp(60.0, 200.0);
        let image = hover.image.clone();
        let time = hover.time;

        div()
            .absolute()
            .left(px(left))
            .bottom(px(22.0))
            .w(px(WIDTH))
            .p(px(PADDING))
            .flex()
            .flex_col()
            .gap(px(PADDING))
            .rounded(px(8.0))
            .bg(surface())
            .border_1()
            .border_color(gpui_kit::rgb(0x2c2c34))
            .shadow_lg()
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(image_height))
                    .rounded(px(5.0))
                    .overflow_hidden()
                    .bg(gpui_kit::rgb(0x000000))
                    .child(
                        canvas(
                            |_bounds, _window, _cx| (),
                            move |bounds, (), window, _cx| {
                                if let Some(image) = image.as_ref() {
                                    let _ = window.paint_image(
                                        bounds,
                                        bounds,
                                        Corners::default(),
                                        image.clone(),
                                        0,
                                        false,
                                    );
                                }
                            },
                        )
                        .absolute()
                        .inset_0(),
                    ),
            )
            .child(
                div()
                    .h(px(LABEL))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.0))
                    .text_color(text())
                    .child(format_time(time)),
            )
            .into_any_element()
    }

    pub(super) fn begin_progress_drag(&mut self, x: Pixels) {
        self.dragging = Some(self.position_for_x(x));
    }

    pub(super) fn update_progress_drag(&mut self, x: Pixels) {
        if self.dragging.is_some() {
            self.dragging = Some(self.position_for_x(x));
        }
    }

    pub(super) fn end_progress_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(position) = self.dragging.take() {
            self.send(Command::SeekAbsolute(position));
            self.poke();
            cx.notify();
        }
    }

    pub(super) fn position_for_x(&self, x: Pixels) -> f64 {
        let bounds = self.progress_bounds.get();
        let width = f32::from(bounds.size.width) as f64;
        if width <= 1.0 || self.snapshot.duration <= 0.0 {
            return self.snapshot.position;
        }
        let offset = f32::from(x) - f32::from(bounds.origin.x);
        let ratio = (offset as f64 / width).clamp(0.0, 1.0);
        ratio * self.snapshot.duration
    }

    // -- 进度条悬停预览 ----------------------------------------------------

    /// 鼠标在进度条上（`Some(x)`）或离开（`None`）时更新预览浮层。
    pub(super) fn update_hover(&mut self, x: Option<Pixels>, cx: &mut Context<Self>) {
        let bounds = self.progress_bounds.get();
        self.update_hover_in(&bounds, x, cx);
    }

    /// 时间轴悬停的通用实现：主控制条与裁切对话框的时间轴共用。
    ///
    /// `bounds` 由调用方给：两条时间轴各有一份几何信息，预览请求与浮层内容
    /// 却是同一套状态（同一时刻只可能悬停在其中一条上）。
    pub(super) fn update_hover_in(
        &mut self,
        bounds: &Bounds<Pixels>,
        x: Option<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(x) = x else {
            if self.hover.is_some() {
                self.drop_hover();
                cx.notify();
            }
            return;
        };

        let duration = self.snapshot.duration;
        let width = f32::from(bounds.size.width);
        if duration <= 0.0 || width <= 1.0 {
            return;
        }

        let offset = (f32::from(x) - f32::from(bounds.origin.x)).clamp(0.0, width);
        let ratio = offset / width;
        let time = ratio as f64 * duration;

        // 只有「手上没有挂着的请求」才发新的。结果回来后会按当时的位置再补一条，
        // 所以最后一格不会漏；而中途那条即使丢掉，用户也早就划过去了。
        // 纯音频没有画面可取，别让预览实例在那儿白解码。
        let pending = self
            .preview_request
            .is_some_and(|(_, at)| at.elapsed() < Duration::from_millis(1500));
        if !pending && self.snapshot.has_video() {
            self.request_preview(time);
        }

        let image = self.hover.as_ref().and_then(|hover| hover.image.clone());
        self.hover = Some(HoverPreview { time, ratio, image });
        cx.notify();
    }

    pub(super) fn request_preview(&mut self, time: f64) {
        let Some(path) = self.snapshot.path.clone() else {
            return;
        };
        let Some(engine) = &self.engine else {
            return;
        };
        engine.preview(&path, time);
        self.preview_request = Some((time, Instant::now()));
    }

    /// 预览图回来了。
    pub(super) fn on_thumbnail(&mut self, time: f64, image: Arc<RenderImage>) {
        self.preview_request = None;

        let Some(hover) = &mut self.hover else {
            // 鼠标已经离开进度条：图没地方放，但它在 atlas 里占着一块，
            // 交给渲染路径统一回收。
            self.preview_retired.push_back(image);
            return;
        };

        if let Some(previous) = hover.image.replace(image) {
            self.preview_retired.push_back(previous);
        }

        // 送来的是几格之前的位置：指针多半已经挪远了，按当前位置再要一张。
        // 鼠标停下后下一次就能对上，所以这个「追」不会无限循环。
        let follow_up = ((hover.time - time).abs() > 0.5).then_some(hover.time);
        if let Some(time) = follow_up {
            self.request_preview(time);
        }
    }

    /// 鼠标离开进度条：浮层收掉。
    ///
    /// 预览实例仍然留着那个文件——回头再划过进度条不用重新打开，
    /// 打开一个文件比 seek 一次贵得多。
    pub(super) fn drop_hover(&mut self) {
        if let Some(hover) = self.hover.take()
            && let Some(image) = hover.image
        {
            self.preview_retired.push_back(image);
        }
        self.preview_request = None;
    }

    /// 换片/关文件：预览整体作废，连预览实例里的文件一起卸掉。
    pub(super) fn clear_preview(&mut self) {
        self.drop_hover();
        if let Some(engine) = &self.engine {
            engine.stop_preview();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 纯音频换片后连新代次的帧也不收() {
        // mpv 会把旧视频的最后一帧按新代次重画出来，只有代次比对挡不住。
        assert!(!frame_accepted(1, 1, true));
        assert!(!frame_accepted(0, 1, false));
        assert!(frame_accepted(1, 1, false));
        // 停止播放、新建空工程时本地代次先推到 PENDING：通道里在途的旧帧一概不收。
        assert!(!frame_accepted(1, FRAME_EPOCH_PENDING, false));
    }

    #[test]
    fn 纯音频播放时不显示空状态占位() {
        let frame: Option<Arc<RenderImage>> = None;
        let mut snapshot = Snapshot::default();
        // 刚启动、或者停止播放之后，仍要提示用户打开文件。
        assert!(show_placeholder(&frame, &snapshot, false));

        // 换到纯音频：帧已清空，视频区应该是纯黑而不是「正在加载…」。
        snapshot.path = Some(std::path::PathBuf::from("lesson.mp3"));
        snapshot.idle = false;
        assert!(!show_placeholder(&frame, &snapshot, false));

        // 换成视频后，解码出第一帧之前照常显示加载提示。
        snapshot.video_width = 1920;
        snapshot.video_height = 1080;
        assert!(show_placeholder(&frame, &snapshot, false));
    }

    #[test]
    fn 工程模式没有帧时保持纯黑() {
        let frame: Option<Arc<RenderImage>> = None;
        let snapshot = Snapshot::default();
        assert!(show_placeholder(&frame, &snapshot, false));
        // 新建的空工程没有任何片段可预览：不显示「拖入视频文件」这类引导语。
        assert!(!show_placeholder(&frame, &snapshot, true));

        // 就算界面快照还留着上一部片子的状态，工程模式下也不该冒出占位。
        let stale = Snapshot {
            path: Some(std::path::PathBuf::from("B.mp4")),
            idle: false,
            video_width: 1920,
            video_height: 1080,
            ..Default::default()
        };
        assert!(!show_placeholder(&frame, &stale, true));
    }
}
