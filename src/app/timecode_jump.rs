//! 跳到时间码：`Ctrl`/`⌘` + `G` 打开，输入时间码把播放头挪过去。
//!
//! 时间码的解析在 `timeline::timecode`（带丢帧），这里只管一个输入框、一句错误
//! 提示和「挪过去之后重新出一帧」。

use super::*;
use crate::timeline::timecode;
use gpui_kit::component::input::{AnyInputState, Input, InputState};

pub(super) struct TimecodeDialog {
    pub(super) input: Entity<InputState>,
    pub(super) error: Option<String>,
}

impl PlayerApp {
    pub(super) fn open_timecode_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            self.flash("先打开工程，再按时间码跳转");
            cx.notify();
            return;
        };
        let fps = project.sequence().fps;
        // 预填当前时刻：多半只是想改最后两位，不用整个敲一遍。
        let initial = timecode::format(project.ui.playhead, fps, timecode::prefers_drop_frame(fps));

        // 模态互斥：叠在一起会抢同一份按键。
        self.close_export_dialog(cx);
        self.close_subtitle_dialog(cx);
        self.close_sequence_dialog(cx);
        self.close_delete_track_confirm(cx);
        self.close_remux_dialog(cx);
        self.close_metadata_dialog(cx);
        self.close_proxy_dialog(cx);
        self.close_scope_dialog(cx);
        self.close_scenes_dialog(cx);
        self.close_multicam_dialog(cx);
        self.close_concat_dialog(cx);
        self.close_clip_dialog(cx);
        self.close_playlist_import_dialog(cx);
        self.close_sequence_manager_dialog(cx);
        self.project_confirm = None;
        self.show_settings = false;
        self.timeline_menu = None;
        self.menu = None;
        self.menu_bar = None;

        let input = cx.new(|cx| InputState::new(window, cx).placeholder("00:00:00:00"));
        input.update(cx, |state, cx| {
            state.set_value(initial, window, cx);
        });
        // 键盘流：打开就能直接敲，不用先点一下输入框再敲。`InputState` 自己的
        // 焦点句柄不对外，借 `AnyInputState` 这个统一入口拿一次。
        let focus = AnyInputState::from(input.clone()).focus_handle(cx);
        focus.focus(window, cx);
        self.timecode_dialog = Some(TimecodeDialog { input, error: None });
        cx.notify();
    }

    pub(super) fn close_timecode_dialog(&mut self, cx: &mut Context<Self>) {
        if self.timecode_dialog.take().is_some() {
            cx.notify();
        }
    }

    /// Esc 关掉、Enter 等于「跳过去」。
    pub(super) fn on_timecode_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if self.timecode_dialog.is_none() {
            return false;
        }
        match event.keystroke.key.as_str() {
            "escape" => {
                self.close_timecode_dialog(cx);
                true
            }
            "enter" => {
                self.confirm_timecode_jump(cx);
                true
            }
            _ => false,
        }
    }

    /// 解析输入并把播放头挪过去。失败只留在面板里说明原因，不清空输入。
    pub(super) fn confirm_timecode_jump(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.timecode_dialog.as_ref() else {
            return;
        };
        let text = dialog.input.read(cx).value().to_string();
        let Some(project) = self.project.as_ref() else {
            self.close_timecode_dialog(cx);
            return;
        };
        let sequence = project.sequence();
        let duration = sequence.duration();
        let fps = sequence.fps;
        match timecode::parse(&text, fps) {
            Ok(parsed) => {
                // 越过序列长度没有意义：停在最后一帧，并说明为什么。
                let clamped = parsed.tick.min(duration);
                if let Some(project) = self.project.as_mut() {
                    project.ui.playhead = clamped;
                }
                self.timecode_dialog = None;
                self.preview_at_playhead(cx);
                if clamped < parsed.tick {
                    self.flash("时间码超过序列长度，停在结尾");
                }
                cx.notify();
            }
            Err(error) => {
                if let Some(dialog) = self.timecode_dialog.as_mut() {
                    dialog.error = Some(error);
                }
                cx.notify();
            }
        }
    }

    pub(super) fn render_timecode_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.timecode_dialog.as_ref()?;
        let fps = self.project.as_ref().map(|project| project.sequence().fps);
        let hint = match fps {
            Some(fps) if timecode::prefers_drop_frame(fps) => {
                "`时:分:秒:帧`，这个帧率是 NTSC，丢帧写法用分号：`00:01:00;02`。".to_string()
            }
            Some(fps) => format!(
                "`时:分:秒:帧`，帧号 0–{}；也可以只写 `分:秒`。",
                (fps.to_f64().round() as i64 - 1).max(0)
            ),
            None => "`时:分:秒:帧`".to_string(),
        };

        let mut card = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_muted())
                    .child(hint),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .child(Input::new(&dialog.input).id("timecode-input").w_full()),
            );
        if let Some(error) = &dialog.error {
            card = card.child(
                div()
                    .text_size(px(11.0))
                    .text_color(ui::danger())
                    .child(error.clone()),
            );
        }
        card = card.child(
            div()
                .flex()
                .justify_end()
                .gap(px(8.0))
                .child(settings_button(
                    "timecode-cancel",
                    "取消",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.close_timecode_dialog(cx);
                    }),
                ))
                .child(settings_button(
                    "timecode-jump",
                    "跳过去",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.confirm_timecode_jump(cx);
                    }),
                )),
        );

        Some(modal_card(
            "timecode-jump-card",
            "跳到时间码",
            card.into_any_element(),
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.close_timecode_dialog(cx);
            }),
        ))
    }
}
