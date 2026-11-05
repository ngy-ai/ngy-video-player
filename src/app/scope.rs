//! 示波器对话框：对当前帧抽样，画出亮度直方图 / 波形图 / 矢量图。
//!
//! 抽样要起一个 ffmpeg 进程，放后台线程；界面先显示「采样中」，结果回来后
//! 再画图。采样期间播放不受影响，用户可以暂停在要检查的那一帧再刷新。

use super::*;
use crate::scope::{self as scope_core, Bitmap, ScopeFrame};

/// 三种视图共用同一份抽样结果，切换不重新抽帧。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScopeView {
    Luma,
    Waveform,
    Vectorscope,
}

impl ScopeView {
    const ALL: [ScopeView; 3] = [ScopeView::Luma, ScopeView::Waveform, ScopeView::Vectorscope];

    fn label(self) -> &'static str {
        match self {
            ScopeView::Luma => "亮度",
            ScopeView::Waveform => "波形",
            ScopeView::Vectorscope => "矢量",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            ScopeView::Luma => "左暗右亮；死黑与过曝占比用于判断曝光是否被截断。",
            ScopeView::Waveform => "横轴是画面左右，纵轴是亮度；某一片贴顶就是过曝。",
            ScopeView::Vectorscope => "越靠外饱和度越高；肤色应落在左上偏红的一条带上。",
        }
    }
}

#[derive(Clone, Debug)]
enum ScopeState {
    Loading,
    /// 一帧的三种统计加起来比另外两个变体大一个量级；
    /// 装进 Box 里，枚举本身就不会被撑大。
    Ready(Box<ScopeFrame>),
    Failed(String),
}

pub(super) struct ScopeDialog {
    generation: u64,
    path: PathBuf,
    /// 这张图对应的播放位置（秒），显示在标题下面。
    time: f64,
    view: ScopeView,
    state: ScopeState,
}

impl PlayerApp {
    pub(super) fn open_scope_dialog(&mut self, cx: &mut Context<Self>) {
        if self.scope.is_some() {
            self.close_scope_dialog(cx);
            return;
        }
        let Some(path) = self.snapshot.path.clone() else {
            self.flash("没有正在播放的文件");
            cx.notify();
            return;
        };
        if !self.snapshot.has_video() {
            self.flash("纯音频文件没有画面");
            cx.notify();
            return;
        }

        self.close_remux_dialog(cx);
        self.close_metadata_dialog(cx);
        self.close_proxy_dialog(cx);
        self.close_clip_dialog(cx);
        self.close_multicam_dialog(cx);
        self.close_scenes_dialog(cx);
        self.close_playlist_import_dialog(cx);
        self.scope_generation += 1;
        self.scope = Some(ScopeDialog {
            generation: self.scope_generation,
            path,
            time: self.snapshot.position.max(0.0),
            view: ScopeView::Luma,
            state: ScopeState::Loading,
        });
        self.menu = None;
        self.menu_bar = None;
        self.refresh_scope(cx);
    }

    pub(super) fn close_scope_dialog(&mut self, cx: &mut Context<Self>) {
        self.scope = None;
        cx.notify();
    }

    pub(super) fn sync_scope_source(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.scope.as_ref() else {
            return;
        };
        if self.snapshot.path.as_deref() != Some(dialog.path.as_path()) {
            self.close_scope_dialog(cx);
        }
    }

    pub(super) fn on_scope_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if self.scope.is_none() {
            return false;
        }
        if event.keystroke.key == "escape" {
            self.close_scope_dialog(cx);
            return true;
        }
        false
    }

    /// 切换视图。数据已经算过，只换一种画法，不重新抽帧。
    fn set_scope_view(&mut self, view: ScopeView, cx: &mut Context<Self>) {
        if let Some(dialog) = self.scope.as_mut() {
            dialog.view = view;
        }
        cx.notify();
    }

    /// 按当前播放位置重新抽样。
    fn refresh_scope(&mut self, cx: &mut Context<Self>) {
        let position = self.snapshot.position.max(0.0);
        let Some(dialog) = self.scope.as_mut() else {
            return;
        };
        dialog.time = position;
        dialog.state = ScopeState::Loading;
        let path = dialog.path.clone();
        let generation = dialog.generation;
        cx.notify();

        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            let _ = tx.unbounded_send(scope_core::sample(&path, position));
        });
        cx.spawn(async move |this, cx| {
            if let Some(result) = rx.next().await {
                let _ = this.update(cx, |this, cx| {
                    let Some(dialog) = this.scope.as_mut() else {
                        return;
                    };
                    if dialog.generation != generation {
                        return;
                    }
                    dialog.state = match result {
                        Ok(scope) => ScopeState::Ready(Box::new(scope)),
                        Err(error) => ScopeState::Failed(format!("{error:#}")),
                    };
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(super) fn render_scope_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.scope.as_ref()?;
        let mut body = div().flex().flex_col().gap(px(10.0)).child(
            div()
                .text_size(px(11.0))
                .text_color(text_muted())
                .child(format!(
                    "{} · {}",
                    file_name(&dialog.path),
                    format_time(dialog.time)
                )),
        );

        if !matches!(dialog.state, ScopeState::Loading) {
            body = body.child(view_chips(dialog.view, cx));
        }

        match &dialog.state {
            ScopeState::Loading => {
                body = body.child(
                    div()
                        .h(px(180.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.0))
                        .text_color(text_faint())
                        .child("正在抽帧统计…"),
                );
            }
            ScopeState::Failed(message) => {
                body = body.child(
                    div()
                        .h(px(180.0))
                        .flex()
                        .items_center()
                        .p(px(10.0))
                        .rounded(px(8.0))
                        .bg(gpui_kit::rgba(0x2a1414f2))
                        .text_size(px(11.0))
                        .text_color(ui::danger())
                        .whitespace_normal()
                        .child(message.clone()),
                );
            }
            ScopeState::Ready(scope) => {
                body = body.child(match dialog.view {
                    ScopeView::Luma => histogram(&scope.luma),
                    ScopeView::Waveform => bitmap_image(scope.waveform_bitmap()),
                    ScopeView::Vectorscope => bitmap_image(scope.vectorscope_bitmap()),
                });
            }
        }

        let body = body
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(text_faint())
                            .whitespace_normal()
                            .child(dialog.view.hint()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(settings_button(
                                "scope-refresh",
                                "按当前位置刷新",
                                true,
                                cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    this.refresh_scope(cx);
                                }),
                            ))
                            .child(primary_button(
                                "scope-close",
                                "关闭",
                                true,
                                cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    this.close_scope_dialog(cx);
                                }),
                            )),
                    ),
            )
            .into_any_element();
        Some(modal_card(
            "scope-dialog",
            "示波器",
            body,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.close_scope_dialog(cx);
            }),
        ))
    }
}

/// 一排视图切换按钮。
fn view_chips(view: ScopeView, cx: &mut Context<PlayerApp>) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .children(
            ScopeView::ALL
                .into_iter()
                .map(|candidate| view_chip(candidate, candidate == view, cx)),
        )
        .into_any_element()
}

fn view_chip(view: ScopeView, active: bool, cx: &mut Context<PlayerApp>) -> AnyElement {
    let id = match view {
        ScopeView::Luma => "scope-view-luma",
        ScopeView::Waveform => "scope-view-waveform",
        ScopeView::Vectorscope => "scope-view-vectorscope",
    };
    chip(
        id,
        view.label(),
        active,
        cx.listener(move |this, _: &ClickEvent, _window, cx| {
            this.set_scope_view(view, cx);
        }),
    )
}

fn histogram(scope: &crate::scope::LumaScope) -> AnyElement {
    let peak = scope.peak().max(1) as f32;
    let mut bars = div()
        .id("scope-bars")
        .flex()
        .items_end()
        .gap(px(1.0))
        .h(px(172.0))
        .w_full()
        .p(px(8.0))
        .rounded(px(8.0))
        .bg(gpui_kit::rgba(0x0f0f13f2));
    for count in scope.bins {
        let ratio = count as f32 / peak;
        // 最矮也留 1px：零计数柱完全消失时，读者分不清「没有」和「画丢了」。
        let height = px((ratio * 156.0).max(1.0));
        bars = bars.child(div().flex_1().h(height).rounded_t(px(1.0)).bg(ui::accent()));
    }

    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(bars)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .text_size(px(10.0))
                .text_color(text_faint())
                .child(format!(
                    "平均 {:>3.0}% · 死黑 {:.1}% · 过曝 {:.1}%",
                    scope.mean() * 100.0,
                    scope.black_ratio() * 100.0,
                    scope.white_ratio() * 100.0
                ))
                .child("暗 ← → 亮"),
        )
        .into_any_element()
}

/// 把示波器位图贴进对话框。位图分辨率固定，缩放交给 GPU。
fn bitmap_image(bitmap: Bitmap) -> AnyElement {
    let aspect = bitmap.width as f32 / bitmap.height.max(1) as f32;
    let buffer: image::ImageBuffer<image::Rgba<u8>, Vec<u8>> =
        image::ImageBuffer::from_raw(bitmap.width, bitmap.height, bitmap.pixels)
            .expect("缓冲区尺寸由我们自己分配，必然匹配");
    let image = Arc::new(RenderImage::new([image::Frame::new(buffer)]));
    div()
        .h(px(180.0))
        .w_full()
        .rounded(px(8.0))
        .overflow_hidden()
        .bg(gpui_kit::rgba(0x0f0f13ff))
        .child(
            canvas(
                |bounds, _window, _cx| bounds,
                move |_bounds, target, window, _cx| {
                    let fitted = ui::fit_rect(target, aspect);
                    let _ = window.paint_image(
                        fitted,
                        fitted,
                        Corners::default(),
                        image.clone(),
                        0,
                        false,
                    );
                },
            )
            .size_full(),
        )
        .into_any_element()
}
