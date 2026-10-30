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
        }
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

        Some(
            div()
                .id("clip-dialog")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui_kit::rgba(0x000000b8))
                // 模态：点遮罩不该穿透去暂停视频。
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_this, _: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                    }),
                )
                .child(
                    div()
                        .id("clip-card")
                        .w(px(460.0))
                        .max_w(gpui_kit::relative(0.92))
                        .p(px(18.0))
                        .rounded(px(14.0))
                        .bg(surface())
                        .border_1()
                        .border_color(gpui_kit::rgb(0x2c2c34))
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(12.0))
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

        body.child(
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
                .child(clip_primary_button(
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
                    .child(clip_primary_button(
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

fn clip_primary_button(
    id: impl Into<ElementId>,
    label: &'static str,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .h(px(32.0))
        .px(px(14.0))
        .rounded(px(8.0))
        .bg(if enabled { ui::accent() } else { ui::track() })
        .text_size(px(12.0))
        .text_color(if enabled { ui::bg() } else { text_faint() })
        .when(enabled, |this| {
            this.cursor(CursorStyle::PointingHand)
                .hover(|style| style.opacity(0.9))
                .on_click(
                    move |event: &ClickEvent, window: &mut Window, cx: &mut App| {
                        cx.stop_propagation();
                        on_click(event, window, cx);
                    },
                )
        })
        .child(label)
        .into_any_element()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
