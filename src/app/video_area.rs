//! 视频区渲染：帧交换、横纵比、进度条与悬停预览。

use super::*;

impl PlayerApp {
    pub(super) fn swap_frame(&mut self, window: &mut Window) {
        let Some(incoming) = self.incoming.take() else {
            return;
        };
        if let Some(previous) = self.frame.replace(incoming) {
            self.retired.push_back(previous);
            while self.retired.len() > 2 {
                if let Some(stale) = self.retired.pop_front() {
                    let _ = window.drop_image(stale);
                }
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
        // 面板占掉的那一条不算视频区域，否则缩画面时会被拉伸。
        let panel = if self.settings.show_playlist {
            self.playlist_width(window)
        } else {
            0.0
        };
        let area_width = (f32::from(viewport.width) - panel).max(2.0) * scale;
        // 窗口模式下菜单栏占掉了顶部一条，渲染目标要扣掉，否则画面会被裁掉一截。
        let chrome = if window.is_fullscreen() {
            0.0
        } else {
            MENUBAR_HEIGHT
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
        if let Some(cap) = self.settings.render_mode.height_cap() {
            if height > cap {
                width = ((width as f32 * cap as f32 / height as f32).round() as u32).max(2);
                height = cap;
            }
        }
        engine.set_render_target(width, height, self.settings.video_rotate.degrees());
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
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id("video-area")
            .absolute()
            .inset_0()
            .top(top)
            // 面板是真正的侧边栏：画面让出它那一条，而不是被盖住。
            .when(panel > 0.0, |this| this.right(px(panel)))
            .bg(bg())
            .child(ui::video_surface(self.frame.clone()))
            // 空状态只看「有没有帧」，不看 mpv 上报的宽高：width/height 属性在
            // 视频解码前是 0，用它会在一段正常播放的画面上盖一层「正在加载」。
            .when(self.frame.is_none(), |this| {
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
            .children(self.hover.as_ref().map(|hover| self.render_preview(hover)))
    }

    /// 进度条上方的悬停预览：一张小图 + 对应时间。
    ///
    /// 浮层是进度条的绝对定位子节点，画在画面之上、控制条之外，
    /// 不占控制条的布局空间，也不会把进度条顶高。
    pub(super) fn render_preview(&self, hover: &HoverPreview) -> AnyElement {
        const WIDTH: f32 = 208.0;
        const PADDING: f32 = 3.0;
        const LABEL: f32 = 18.0;

        let bar_width = f32::from(self.progress_bounds.get().size.width);
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
        let Some(x) = x else {
            if self.hover.is_some() {
                self.drop_hover();
                cx.notify();
            }
            return;
        };

        let duration = self.snapshot.duration;
        let bounds = self.progress_bounds.get();
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
