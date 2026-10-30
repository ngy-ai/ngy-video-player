//! 裁切片段对话框：两个时间输入框、进度与完成后的操作。
//!
//! 输入框是自绘的：播放器整套界面都是 `div` 拼的，为两个只输数字的字段引入组件库
//! 的输入框，样式和主题都要跟着搬，不划算。代价是没有选区与输入法——时间只需要
//! 数字、冒号和小数点，够用。

use super::*;
use crate::clip::{
    self as clip_core, ClipEvent, ClipJob, ClipOutcome, ClipRange, ClipRequest, RangeError,
};
use std::path::Path;

/// 输入框里最多放得下 `01:02:03.500` 这样的内容。
const MAX_INPUT_CHARS: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClipField {
    Start,
    End,
}

/// 对话框当前处在哪一步。
#[derive(Clone, Debug)]
enum ClipState {
    Editing,
    Running { progress: f32 },
    Done(PathBuf),
    Failed(String),
}

/// 裁切对话框的全部状态。
pub(super) struct ClipDialog {
    /// 任务代次：后台任务回来时用它确认「这个事件还属于当前对话框」。
    generation: u64,
    path: PathBuf,
    duration: f64,
    start: String,
    end: String,
    field: ClipField,
    state: ClipState,
    job: Option<Arc<ClipJob>>,
    /// 拖动时间轴时的临时位置（秒）；`None` 表示没在拖。
    ///
    /// 拖动过程里不逐像素 seek：mpv 每次 seek 都要重新解码，拖快了画面会追不上，
    /// 也让进度事件挤成一团。松手才真正跳过去（与主控制条的进度条一致）。
    scrub: Option<f64>,
    /// 时间轴的 bounds，由 canvas 在 prepaint 阶段写回。
    bounds: Rc<Cell<Bounds<Pixels>>>,
    /// 正在预览片段时的终点与开始时刻（秒）。
    ///
    /// 记下开始时刻是为了躲开 seek 的延迟：刚跳到起点时位置事件可能还是旧的
    /// （甚至已经超过终点），不能拿它判断「播到结尾了」。
    preview: Option<(f64, Instant)>,
}

impl ClipDialog {
    fn new(generation: u64, path: PathBuf, duration: f64) -> Self {
        Self {
            generation,
            path,
            duration,
            start: String::new(),
            end: String::new(),
            field: ClipField::Start,
            state: ClipState::Editing,
            job: None,
            scrub: None,
            bounds: Rc::new(Cell::new(Bounds::default())),
            preview: None,
        }
    }

    /// 时间轴当前显示的位置：拖动时跟手，否则跟随播放进度。
    fn scrub_position(&self, playing: f64) -> f64 {
        let position = self.scrub.unwrap_or(playing);
        position.clamp(0.0, self.duration.max(0.0))
    }

    fn editing(&self) -> bool {
        matches!(self.state, ClipState::Editing | ClipState::Failed(_))
    }

    /// 当前输入能构成什么区间。`Ok(None)` 表示两端都留空。
    fn range(&self) -> Result<Option<ClipRange>, RangeError> {
        clip_core::range_from_inputs(&self.start, &self.end, self.duration)
    }

    fn active_text_mut(&mut self) -> &mut String {
        match self.field {
            ClipField::Start => &mut self.start,
            ClipField::End => &mut self.end,
        }
    }
}

impl PlayerApp {
    /// 打开对话框。没有正在播放的文件、或者时长还没读出来时，只给一条提示。
    pub(super) fn open_clip_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.snapshot.path.clone() else {
            self.flash("没有正在播放的文件");
            cx.notify();
            return;
        };
        let duration = self.snapshot.duration;
        if duration <= 0.0 {
            self.flash("还没读出这个文件的时长，稍等一下再试");
            cx.notify();
            return;
        }

        // 两个对话框都是模态的，叠在一起会互相挡住，也会抢同一份按键。
        self.close_remux_dialog(cx);
        self.clip_generation += 1;
        self.clip = Some(ClipDialog::new(self.clip_generation, path, duration));
        // 和设置窗口一致：菜单叠在模态面板上只会互相挡住。
        self.menu = None;
        self.menu_bar = None;
        cx.notify();
    }

    /// 关闭对话框。正在跑的任务一并取消，半成品由任务线程删掉。
    pub(super) fn close_clip_dialog(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = &self.clip
            && let Some(job) = &dialog.job
        {
            job.cancel();
        }
        self.clip = None;
        // 悬停预览的浮层挂在 `self.hover` 上：不收拾掉，主控制条那边会接着画。
        self.drop_hover();
        cx.notify();
    }

    /// 对话框打开时接管键盘，返回 `true` 表示这次按键已被消费。
    pub(super) fn on_clip_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(dialog) = self.clip.as_ref() else {
            return false;
        };

        if event.keystroke.key == "escape" {
            self.close_clip_dialog(cx);
            return true;
        }
        // 运行中除 Esc 外一律吞掉：空格、方向键这些是播放快捷键，不能漏下去。
        if !dialog.editing() {
            return true;
        }

        match event.keystroke.key.as_str() {
            "tab" | "down" => {
                self.clip_field_switch(cx, false);
                true
            }
            "up" => {
                self.clip_field_switch(cx, true);
                true
            }
            "enter" => {
                self.start_clip(cx);
                true
            }
            // 选点要能看画面：编辑输入框时也放开播放控制的几个键。
            // 空格在时间输入里没有意义，左右键也不用作文本光标（输入永远是追加/退格）。
            "space" => {
                self.toggle_play(cx);
                true
            }
            // Alt + 左右键逐帧：恰好卡在某帧上时，5 秒的步长太粗。
            "left" if event.keystroke.modifiers.alt => {
                self.step_frame(-1, cx);
                true
            }
            "right" if event.keystroke.modifiers.alt => {
                self.step_frame(1, cx);
                true
            }
            "left" => {
                self.seek_by(-5.0, cx);
                true
            }
            "right" => {
                self.seek_by(5.0, cx);
                true
            }
            "backspace" => {
                if let Some(dialog) = self.clip.as_mut() {
                    dialog.active_text_mut().pop();
                    cx.notify();
                }
                true
            }
            _ => {
                let typed = event.keystroke.key_char.as_deref().filter(|text| {
                    !text.is_empty()
                        && text
                            .chars()
                            .all(|ch| ch.is_ascii_digit() || ch == ':' || ch == '.')
                });
                if let Some(text) = typed
                    && let Some(dialog) = self.clip.as_mut()
                {
                    let target = dialog.active_text_mut();
                    if target.len() + text.len() <= MAX_INPUT_CHARS {
                        target.push_str(text);
                        cx.notify();
                    }
                }
                // 对话框是模态的：认不出的键也不让下层继续处理。
                true
            }
        }
    }

    fn clip_field_switch(&mut self, cx: &mut Context<Self>, backwards: bool) {
        let Some(dialog) = self.clip.as_mut() else {
            return;
        };
        dialog.field = match (dialog.field, backwards) {
            (ClipField::Start, false) | (ClipField::End, true) => ClipField::End,
            (ClipField::End, false) | (ClipField::Start, true) => ClipField::Start,
        };
        cx.notify();
    }

    /// 时间轴上的横坐标换算成秒。拿不到宽度时退回当前播放位置。
    fn clip_position_for_x(&self, x: Pixels) -> f64 {
        let Some(dialog) = self.clip.as_ref() else {
            return self.snapshot.position;
        };
        let bounds = dialog.bounds.get();
        let width = f32::from(bounds.size.width) as f64;
        if width <= 1.0 || dialog.duration <= 0.0 {
            return self.snapshot.position;
        }
        let offset = f32::from(x) - f32::from(bounds.origin.x);
        let ratio = (offset as f64 / width).clamp(0.0, 1.0);
        ratio * dialog.duration
    }

    fn clip_begin_seek(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let position = self.clip_position_for_x(x);
        if let Some(dialog) = self.clip.as_mut() {
            dialog.scrub = Some(position);
        }
        cx.notify();
    }

    fn clip_update_seek(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let position = self.clip_position_for_x(x);
        let Some(dialog) = self.clip.as_mut() else {
            return;
        };
        if dialog.scrub.is_some() {
            dialog.scrub = Some(position);
            cx.notify();
        }
    }

    fn clip_end_seek(&mut self, cx: &mut Context<Self>) {
        let Some(position) = self.clip.as_mut().and_then(|dialog| dialog.scrub.take()) else {
            return;
        };
        // 松手才真正跳过去：拖动中逐像素 seek 会让画面一直追不上手指。
        self.send(Command::SeekAbsolute(position));
        self.poke();
        cx.notify();
    }

    /// 把时间轴当前的位置写进开始/结束输入框。
    fn clip_mark(&mut self, field: ClipField, cx: &mut Context<Self>) {
        let playing = self.snapshot.position;
        let Some(dialog) = self.clip.as_mut() else {
            return;
        };
        let position = dialog.scrub_position(playing);
        let text = clip_core::format_time(position);
        let text: String = text.chars().take(MAX_INPUT_CHARS).collect();
        match field {
            ClipField::Start => dialog.start = text,
            ClipField::End => dialog.end = text,
        }
        // 焦点跟到刚填的字段，接着用键盘微调或直接改都顺手。
        dialog.field = field;
        cx.notify();
    }

    /// 时间轴悬停：预览浮层与缩略图请求复用主控制条那套状态。
    fn update_clip_hover(&mut self, x: Option<Pixels>, cx: &mut Context<Self>) {
        let Some(bounds) = self.clip.as_ref().map(|dialog| dialog.bounds.get()) else {
            return;
        };
        self.update_hover_in(&bounds, x, cx);
    }

    /// 预览选定区间：从开始播到结束，带声音，方便用耳朵和眼睛确认起止点。
    fn clip_preview_range(&mut self, cx: &mut Context<Self>) {
        let range = match self.clip.as_ref().map(|dialog| dialog.range()) {
            Some(Ok(Some(range))) => range,
            Some(Err(error)) => {
                self.flash(error.message());
                cx.notify();
                return;
            }
            _ => {
                self.flash("先设置开始和结束，才能预览片段");
                cx.notify();
                return;
            }
        };
        if let Some(dialog) = self.clip.as_mut() {
            dialog.preview = Some((range.end, Instant::now()));
        }
        self.snapshot.paused = false;
        self.send(Command::SeekAbsolute(range.start));
        self.send(Command::SetPaused(false));
        self.poke();
        cx.notify();
    }

    /// 预览播到终点就自动停下。由 `render` 每帧检查——播放中画面刷新会带着它走。
    pub(super) fn poll_clip_preview(&mut self, cx: &mut Context<Self>) {
        let Some((end, started)) = self.clip.as_ref().and_then(|dialog| dialog.preview) else {
            return;
        };
        // 用户自己按了暂停：不再替他停第二次，也不再跟踪终点。
        if self.snapshot.paused {
            if let Some(dialog) = self.clip.as_mut() {
                dialog.preview = None;
            }
            return;
        }
        // 刚发出 seek 时位置事件可能还是旧的，先给 seek 一点时间。
        if started.elapsed() < Duration::from_millis(250) || self.snapshot.position < end - 0.02 {
            return;
        }
        if let Some(dialog) = self.clip.as_mut() {
            dialog.preview = None;
        }
        self.snapshot.paused = true;
        self.send(Command::SetPaused(true));
        self.flash("预览结束");
        cx.notify();
    }

    /// 起一个后台任务。参数校验失败时留在编辑态，把原因显示在对话框里。
    pub(super) fn start_clip(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.clip.as_mut() else {
            return;
        };
        if !dialog.editing() {
            return;
        }
        let range = match dialog.range() {
            Ok(Some(range)) => range,
            // 两端都留空不是「复制一份」：按钮本来就是禁用的，这里再挡一次。
            Ok(None) | Err(_) => return,
        };

        let request = ClipRequest {
            output: clip_core::output_path(&dialog.path),
            input: dialog.path.clone(),
            range,
        };
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        match ClipJob::start(request, tx) {
            Ok(job) => {
                dialog.job = Some(Arc::new(job));
                dialog.state = ClipState::Running { progress: 0.0 };
                // 正在预览的那一段不该在裁切跑起来之后再把视频按停。
                dialog.preview = None;
            }
            // 进程都没起来，不会有任何事件，别去等一个空通道。
            Err(error) => {
                dialog.state = ClipState::Failed(format!("{error:#}"));
                cx.notify();
                return;
            }
        }
        let generation = dialog.generation;
        cx.notify();

        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let _ = this.update(cx, |this, cx| {
                    let Some(dialog) = this.clip.as_mut() else {
                        return;
                    };
                    // 旧任务可能在新对话框打开之后才收尾：只认当前代次。
                    if dialog.generation != generation {
                        return;
                    }
                    match event {
                        ClipEvent::Progress(progress) => {
                            if let ClipState::Running { progress: current } = &mut dialog.state {
                                *current = progress;
                            }
                        }
                        ClipEvent::Done(outcome) => {
                            dialog.job = None;
                            dialog.state = match outcome {
                                ClipOutcome::Finished(path) => ClipState::Done(path),
                                ClipOutcome::Cancelled => ClipState::Editing,
                                ClipOutcome::Failed(message) => ClipState::Failed(message),
                            };
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn clip_open_result(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.clip.as_ref() else {
            return;
        };
        let ClipState::Done(path) = &dialog.state else {
            return;
        };
        match platform::open::open(path) {
            Ok(()) => {
                let path = path.clone();
                self.flash(format!("已打开 {}", path.display()));
            }
            Err(message) => self.flash(message),
        }
        cx.notify();
    }

    fn clip_reveal_result(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.clip.as_ref() else {
            return;
        };
        let ClipState::Done(path) = &dialog.state else {
            return;
        };
        let path = path.clone();
        match platform::reveal::reveal(&path) {
            Ok(()) => self.flash("已在文件管理器里定位片段"),
            Err(message) => self.flash(message),
        }
        cx.notify();
    }

    /// 对话框本体。放在模态层最上面，遮住底下的视频与菜单。
    pub(super) fn render_clip_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.clip.as_ref()?;
        let body = match &dialog.state {
            ClipState::Editing | ClipState::Failed(_) => self.render_clip_editor(dialog, cx),
            ClipState::Running { progress } => self.render_clip_progress(*progress, cx),
            ClipState::Done(path) => self.render_clip_done(path, cx),
        };

        Some(modal_card(
            "clip-dialog",
            "裁切片段",
            body,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.close_clip_dialog(cx);
            }),
        ))
    }

    fn render_clip_editor(&self, dialog: &ClipDialog, cx: &mut Context<Self>) -> AnyElement {
        let name = file_name(&dialog.path);
        let range = dialog.range();
        // 两端都留空时按钮不可用：这不是「复制整份文件」的入口。
        let can_start = matches!(range, Ok(Some(_)));
        let hint: Option<(String, bool)> = match &range {
            Ok(None) => Some(("开始和结束都留空：请至少填写一端".to_string(), false)),
            Ok(Some(range)) => Some((
                format!(
                    "{} → {}，共 {}",
                    clip_core::format_time(range.start),
                    clip_core::format_time(range.end),
                    clip_core::format_time(range.duration())
                ),
                true,
            )),
            Err(error) => Some((error.message().to_string(), false)),
        };

        let mut body = div().flex().flex_col().gap(px(10.0)).child(
            div()
                .text_size(px(12.0))
                .text_color(text_muted())
                .child(ui::ellipsis(format!(
                    "{name} · 全长 {}",
                    clip_core::format_time(dialog.duration)
                ))),
        );

        // 失败原因来自 ffmpeg，可能有多行；单独一块红字，别和输入校验混在一起。
        if let ClipState::Failed(message) = &dialog.state {
            body = body.child(
                div()
                    .p(px(10.0))
                    .rounded(px(8.0))
                    .bg(gpui_kit::rgba(0x2a1414f2))
                    .text_size(px(11.0))
                    .text_color(ui::danger())
                    // ffmpeg 的错误是多行的，别挤成一行。
                    .whitespace_normal()
                    .child(message.clone()),
            );
        }

        body.child(self.render_clip_scrubber(dialog, cx))
            .child(
                div()
                    .flex()
                    .gap(px(10.0))
                    .child(self.render_clip_field(
                        ClipField::Start,
                        "开始",
                        &dialog.start,
                        "留空 = 从头",
                        cx,
                    ))
                    .child(self.render_clip_field(
                        ClipField::End,
                        "结束",
                        &dialog.end,
                        "留空 = 到最后",
                        cx,
                    )),
            )
            .when_some(hint, |this, (text, ok)| {
                this.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(if ok { ui::accent() } else { ui::danger() })
                        .child(text),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.0))
                    .child(settings_button(
                        "clip-cancel",
                        "取消",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.close_clip_dialog(cx);
                        }),
                    ))
                    .child(primary_button(
                        "clip-start",
                        "开始裁切",
                        can_start,
                        cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.start_clip(cx);
                        }),
                    )),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(text_faint())
                    .child("精确重编码为 H.264 + AAC 的 mkv，耗时与片长相关；开始后可随时取消。"),
            )
            .into_any_element()
    }

    /// 时间轴：拖动定位、松手 seek，并把当前位置一键填进开始/结束。
    ///
    /// 轨道上还会压一层浅色标出已选的区间。输入框里的数字是精确值，但「这段在
    /// 片子的哪个位置」看图更直接。
    fn render_clip_scrubber(&self, dialog: &ClipDialog, cx: &mut Context<Self>) -> AnyElement {
        let duration = dialog.duration.max(0.001);
        let position = dialog.scrub_position(self.snapshot.position);
        let ratio = (position / duration).clamp(0.0, 1.0) as f32;
        let dragging = dialog.scrub.is_some();
        let slot = dialog.bounds.clone();
        // 上一帧的宽度：缩略图浮层按它定位，canvas 本帧写回的新值下一帧才用得上。
        let bar_width = f32::from(dialog.bounds.get().size.width);
        let selected = dialog.range().ok().flatten();

        let mut track = div()
            .relative()
            .h(px(6.0))
            .w_full()
            .rounded(px(3.0))
            .bg(ui::track());
        if let Some(range) = selected {
            let left = (range.start / duration).clamp(0.0, 1.0) as f32;
            let width = (range.duration() / duration).clamp(0.0, 1.0) as f32;
            track = track.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(gpui_kit::relative(left))
                    .w(gpui_kit::relative(width))
                    .rounded(px(3.0))
                    .bg(gpui_kit::rgba(0x5aa9ff4d)),
            );
        }
        track = track.child(
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
                        .top(px(-2.5))
                        .size(px(if dragging { 13.0 } else { 11.0 }))
                        .rounded_full()
                        .bg(ui::accent()),
                ),
        );

        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text_muted())
                            .child("播放位置"),
                    )
                    .child(div().text_size(px(11.0)).text_color(text()).child(format!(
                        "{} / {}",
                        clip_core::format_time(position),
                        clip_core::format_time(dialog.duration)
                    ))),
            )
            .child(
                div()
                    .id("clip-scrub")
                    .relative()
                    .h(px(18.0))
                    .w_full()
                    .cursor(CursorStyle::PointingHand)
                    .child(
                        gpui_kit::canvas(
                            move |bounds, _window, _cx| slot.set(bounds),
                            |_bounds, (), _window, _cx| {},
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(track)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            cx.stop_propagation();
                            this.clip_begin_seek(event.position.x, cx);
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseUpEvent, _window, cx| {
                            this.clip_end_seek(cx);
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                        this.clip_update_seek(event.position.x, cx);
                        this.update_clip_hover(Some(event.position.x), cx);
                    }))
                    // 指针移出就只能靠 `on_hover` 得知，否则缩略图会一直挂着。
                    .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                        if !*hovered {
                            this.update_clip_hover(None, cx);
                        }
                    }))
                    .children(
                        self.hover
                            .as_ref()
                            .map(|hover| self.render_preview(hover, bar_width)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(settings_button(
                        "clip-mark-start",
                        "设为开始",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.clip_mark(ClipField::Start, cx);
                        }),
                    ))
                    .child(settings_button(
                        "clip-mark-end",
                        "设为结束",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.clip_mark(ClipField::End, cx);
                        }),
                    ))
                    .child(div().flex_1())
                    .child(settings_button(
                        "clip-preview-range",
                        "预览片段",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.clip_preview_range(cx);
                        }),
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(settings_button(
                        "clip-frame-back",
                        "上一帧",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.step_frame(-1, cx);
                        }),
                    ))
                    .child(settings_button(
                        "clip-toggle-play",
                        if self.snapshot.paused {
                            "播放"
                        } else {
                            "暂停"
                        },
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.toggle_play(cx);
                        }),
                    ))
                    .child(settings_button(
                        "clip-frame-step",
                        "下一帧",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.step_frame(1, cx);
                        }),
                    ))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(text_faint())
                            .child("Alt+←/→ 逐帧，←/→ 5 秒"),
                    ),
            )
            .into_any_element()
    }

    fn render_clip_field(
        &self,
        field: ClipField,
        label: &'static str,
        value: &str,
        placeholder: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self
            .clip
            .as_ref()
            .is_some_and(|dialog| dialog.field == field);
        let id = match field {
            ClipField::Start => "clip-start-input",
            ClipField::End => "clip-end-input",
        };
        let value = value.to_string();

        div()
            .flex_1()
            .min_w(px(0.0))
            .flex()
            .flex_col()
            .gap(px(5.0))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_muted())
                    .child(label),
            )
            .child(
                div()
                    .id(id)
                    .h(px(34.0))
                    .flex()
                    .items_center()
                    .px(px(10.0))
                    .rounded(px(8.0))
                    .bg(ui::surface_active())
                    .border_1()
                    .border_color(if active { ui::accent() } else { ui::track() })
                    .cursor(CursorStyle::IBeam)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        if let Some(dialog) = this.clip.as_mut() {
                            dialog.field = field;
                        }
                        cx.notify();
                    }))
                    .child(if value.is_empty() {
                        div()
                            .text_size(px(13.0))
                            .text_color(text_faint())
                            .child(placeholder)
                            .into_any_element()
                    } else {
                        div()
                            .text_size(px(13.0))
                            .text_color(text())
                            .child(value)
                            .into_any_element()
                    })
                    // 光标固定画在末尾：这里不做左右移动与选区，输入永远是追加/退格。
                    .when(active, |this| {
                        this.child(div().ml(px(1.0)).w(px(1.0)).h(px(14.0)).bg(ui::accent()))
                    }),
            )
            .into_any_element()
    }

    fn render_clip_progress(&self, progress: f32, cx: &mut Context<Self>) -> AnyElement {
        let percent = (progress * 100.0).round().clamp(0.0, 100.0) as i32;
        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(text_muted())
                    .child(format!("正在重编码… {percent}%")),
            )
            .child(
                div()
                    .h(px(6.0))
                    .w_full()
                    .rounded_full()
                    .bg(ui::track())
                    .child(
                        div()
                            .h_full()
                            .rounded_full()
                            .bg(ui::accent())
                            .w(gpui_kit::relative(progress.clamp(0.0, 1.0))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .child(settings_button(
                        "clip-abort",
                        "取消",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            // 关闭即取消：任务线程负责杀掉 ffmpeg 并删掉半成品。
                            this.close_clip_dialog(cx);
                        }),
                    )),
            )
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(text_faint())
                    .child("裁切在后台进行，关掉对话框或按 Esc 会取消并删除半成品。"),
            )
            .into_any_element()
    }

    fn render_clip_done(&self, path: &Path, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(text())
                    .child("片段已保存"),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_muted())
                    .whitespace_normal()
                    .child(path.display().to_string()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.0))
                    .child(settings_button(
                        "clip-reveal",
                        "定位",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.clip_reveal_result(cx);
                        }),
                    ))
                    .child(settings_button(
                        "clip-open",
                        "打开",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.clip_open_result(cx);
                        }),
                    ))
                    .child(primary_button(
                        "clip-finish",
                        "完成",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.close_clip_dialog(cx);
                        }),
                    )),
            )
            .into_any_element()
    }
}
