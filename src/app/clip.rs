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

/// 裁切面板的全部状态。
pub(super) struct ClipDialog {
    /// 任务代次：后台任务回来时用它确认「这个事件还属于当前对话框」。
    generation: u64,
    path: PathBuf,
    duration: f64,
    start: String,
    end: String,
    /// 当前有输入焦点的字段；`None` 表示键盘归播放器。
    ///
    /// 面板不是模态的：没点进输入框时，空格、方向键必须照常控制播放——
    /// 用户要一边拖主进度条一边看画面、听声音来定位置。
    field: Option<ClipField>,
    state: ClipState,
    job: Option<Arc<ClipJob>>,
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
            field: None,
            state: ClipState::Editing,
            job: None,
            preview: None,
        }
    }

    fn editing(&self) -> bool {
        matches!(self.state, ClipState::Editing | ClipState::Failed(_))
    }

    /// 当前输入能构成什么区间。`Ok(None)` 表示两端都留空。
    fn range(&self) -> Result<Option<ClipRange>, RangeError> {
        clip_core::range_from_inputs(&self.start, &self.end, self.duration)
    }

    /// 有焦点的输入框；`None` 时不该有键盘输入进来。
    fn active_text_mut(&mut self) -> Option<&mut String> {
        match self.field? {
            ClipField::Start => Some(&mut self.start),
            ClipField::End => Some(&mut self.end),
        }
    }
}

impl PlayerApp {
    /// 打开对话框。没有正在播放的文件、或者时长还没读出来时，只给一条提示。
    pub(super) fn open_clip_dialog(&mut self, cx: &mut Context<Self>) {
        // 再点一次菜单就是收起：面板不挡播放，开关语义比「重新开一个」自然。
        if self.clip.is_some() {
            self.close_clip_dialog(cx);
            return;
        }
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

        // 轨道编辑还是模态的，叠在面板上会挡住画面与进度条。
        self.close_remux_dialog(cx);
        // 转换面板与裁切抢同一条右侧通道，先让位（正在跑的任务一并取消）。
        self.dismiss_convert_panel();
        self.clip_generation += 1;
        self.clip = Some(ClipDialog::new(self.clip_generation, path, duration));
        // 播放列表与裁切面板抢同一条右侧通道，先让位。
        self.set_playlist_visible(false);
        self.menu = None;
        self.menu_bar = None;
        cx.notify();
    }

    /// 关闭面板。正在跑的任务一并取消，半成品由任务线程删掉。
    pub(super) fn close_clip_dialog(&mut self, cx: &mut Context<Self>) {
        self.dismiss_clip_panel();
        cx.notify();
    }

    /// 换片后裁切面板不能再留：它的时长与路径都来自打开时那一个文件，
    /// 继续用会把新片的播放位置写进旧片的区间。由 `render` 每帧对照一次。
    pub(super) fn sync_clip_source(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.clip.as_ref() else {
            return;
        };
        if self.snapshot.path.as_deref() != Some(dialog.path.as_path()) {
            self.close_clip_dialog(cx);
        }
    }

    /// 收面板但不刷新。给同一帧里还要继续改状态的调用方（切播放列表）。
    pub(super) fn dismiss_clip_panel(&mut self) {
        if let Some(dialog) = self.clip.take()
            && let Some(job) = &dialog.job
        {
            job.cancel();
        }
    }

    /// 输入框有焦点时才接管键盘，返回 `true` 表示这次按键已被消费。
    ///
    /// 面板不再是模态的：没点进输入框时，空格、方向键、Ctrl/⌘+左右这些必须原样
    /// 留给播放器——用户正是一边看画面一边用它们定位。
    pub(super) fn on_clip_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(dialog) = self.clip.as_ref() else {
            return false;
        };
        if dialog.field.is_none() || !dialog.editing() {
            return false;
        }

        match event.keystroke.key.as_str() {
            // Esc 先退出输入焦点，再按一次才收面板（走 ExitOverlay）。
            "escape" => {
                if let Some(dialog) = self.clip.as_mut() {
                    dialog.field = None;
                }
                cx.notify();
                true
            }
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
            "backspace" => {
                if let Some(dialog) = self.clip.as_mut()
                    && let Some(text) = dialog.active_text_mut()
                {
                    text.pop();
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
                let Some(text) = typed else {
                    // 空格、方向键之类不参与时间输入，交还给播放器。
                    return false;
                };
                if let Some(dialog) = self.clip.as_mut()
                    && let Some(target) = dialog.active_text_mut()
                    && target.len() + text.len() <= MAX_INPUT_CHARS
                {
                    target.push_str(text);
                }
                cx.notify();
                true
            }
        }
    }

    fn clip_field_switch(&mut self, cx: &mut Context<Self>, backwards: bool) {
        let Some(dialog) = self.clip.as_mut() else {
            return;
        };
        dialog.field = match (dialog.field, backwards) {
            (Some(ClipField::Start), false) | (Some(ClipField::End), true) => Some(ClipField::End),
            _ => Some(ClipField::Start),
        };
        cx.notify();
    }

    /// 把播放器当前的位置写进开始/结束输入框。
    ///
    /// 面板与播放器共用主进度条：用户拖进度条、逐帧微调之后停在哪一帧，
    /// 「设为开始」就取哪一帧，不必在面板里再定位一次。
    fn clip_mark(&mut self, field: ClipField, cx: &mut Context<Self>) {
        let position = self
            .snapshot
            .position
            .clamp(0.0, self.snapshot.duration.max(0.0));
        let text = clip_core::format_time(position);
        let text: String = text.chars().take(MAX_INPUT_CHARS).collect();
        let Some(dialog) = self.clip.as_mut() else {
            return;
        };
        match field {
            ClipField::Start => dialog.start = text,
            ClipField::End => dialog.end = text,
        }
        // 焦点跟到刚填的字段，接着用键盘微调或直接改都顺手。
        dialog.field = Some(field);
        cx.notify();
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

    /// 右侧面板本体。与播放列表共用一条通道：视频和控制条按它让位，
    /// 所以面板不会压住进度条，用户可以直接用主进度条选点。
    pub(super) fn render_clip_panel(
        &self,
        top: Pixels,
        width: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.clip.as_ref()?;
        let body = match &dialog.state {
            ClipState::Editing | ClipState::Failed(_) => self.render_clip_editor(dialog, cx),
            ClipState::Running { progress } => self.render_clip_progress(*progress, cx),
            ClipState::Done(path) => self.render_clip_done(path, cx),
        };

        Some(
            div()
                .id("clip-panel")
                .absolute()
                .top(top)
                .right_0()
                .bottom_0()
                .w(px(width))
                .flex()
                .flex_col()
                .bg(gpui_kit::rgba(0x131317e6))
                .border_l_1()
                .border_color(gpui_kit::rgb(0x26262e))
                // 面板上的点击不该漏到画面去（画面那一层负责播放/暂停）。
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                    if this.item_menu.take().is_some() {
                        cx.notify();
                    }
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .pl(px(14.0))
                        .pr(px(6.0))
                        .py(px(8.0))
                        .child(
                            div()
                                .text_size(px(14.0))
                                .text_color(text())
                                .child("裁切片段"),
                        )
                        .child(icon_button(
                            "clip-close",
                            IconName::X,
                            false,
                            false,
                            cx.listener(|this, _: &ClickEvent, _window, cx| {
                                this.close_clip_dialog(cx);
                            }),
                        )),
                )
                .child(
                    div()
                        .id("clip-panel-body")
                        .flex_1()
                        .min_h(px(0.0))
                        // 窗口矮时内容可滚动，按钮不会掉到面板外面。
                        .overflow_y_scroll()
                        .px(px(14.0))
                        .pb(px(14.0))
                        .child(body),
                )
                .into_any_element(),
        )
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

        // 当前进度用和「全长」同一套格式：拖完进度条、逐帧微调以后，眼睛要能
        // 立刻核对停在哪一帧，控制条上的 mm:ss 精度不够。
        let position = self.snapshot.position.clamp(0.0, dialog.duration.max(0.0));

        let mut body = div().flex().flex_col().gap(px(12.0)).child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(text())
                        .child(ui::ellipsis(name)),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(text_muted())
                        .child(format!("全长 {}", clip_core::format_time(dialog.duration))),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(text_muted())
                        .child(format!("当前 {}", clip_core::format_time(position))),
                ),
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

        body = body.child(
            div()
                .flex()
                .flex_col()
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
        );

        if let Some((text, ok)) = hint {
            body = body.child(
                div()
                    .text_size(px(11.0))
                    .text_color(if ok { ui::accent() } else { ui::danger() })
                    .child(text),
            );
        }

        body.child(
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
                )),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .child(settings_button(
                    "clip-preview-range",
                    "预览片段",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.clip_preview_range(cx);
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
                .text_color(text_muted())
                .whitespace_normal()
                .child(
                    "用控制条上的进度条定位，停在哪一帧就点「设为开始 / 设为结束」。\
                     空格播放/暂停，←/→ 5 秒，Ctrl/⌘+←/→ 逐帧。",
                ),
        )
        .child(
            div()
                .text_size(px(10.0))
                .text_color(text_faint())
                .whitespace_normal()
                .child("精确重编码为 H.264 + AAC 的 mkv，耗时与片长相关；开始后可取消。"),
        )
        .into_any_element()
    }

    fn render_clip_field(
        &self,
        field: ClipField,
        label: &'static str,
        value: &str,
        hint: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self
            .clip
            .as_ref()
            .is_some_and(|dialog| dialog.field == Some(field));
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
            // 提示放在标题行右侧，不再塞进输入框：框里留着占位文字时，光标一进去
            // 就成了「留空 = 从头|」，看着像已经输入了内容，又退不掉。
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text_muted())
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(text_faint())
                            .child(hint),
                    ),
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
                    // 用 mouse_down 而不是 click：根节点在同一次按下时会清焦点，
                    // 这里先截断冒泡，焦点才落得住。
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                            cx.stop_propagation();
                            if let Some(dialog) = this.clip.as_mut() {
                                dialog.field = Some(field);
                            }
                            cx.notify();
                        }),
                    )
                    // 空就是空：留空的含义已经在标题行，框里只放真正输入的内容，
                    // 光标才有地方落。
                    .when(!value.is_empty(), |this| {
                        this.child(div().text_size(px(13.0)).text_color(text()).child(value))
                    })
                    // 光标固定画在末尾：这里不做左右移动与选区，输入永远是追加/退格。
                    .when(active, |this| {
                        this.child(div().ml(px(1.0)).w(px(1.0)).h(px(14.0)).bg(ui::accent()))
                    }),
            )
            .into_any_element()
    }

    /// 点面板以外的地方：输入框失焦，键盘还给播放器。返回是否真的清掉了焦点。
    pub(super) fn clear_clip_focus(&mut self) -> bool {
        let Some(dialog) = self.clip.as_mut() else {
            return false;
        };
        dialog.field.take().is_some()
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
                    .child("裁切在后台进行，关掉面板会取消并删除半成品。"),
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
