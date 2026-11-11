//! 序列管理：一个工程里多条序列的新建 / 切换 / 改名 / 删除。
//!
//! 切换序列是一次「换播放源」：预览时钟、已加载的素材、试听、选中与撤销栈都属于
//! 上一条序列，全部复位后再按新序列刷新一帧。播放头 / 选中 / 工作区在工程里也只有
//! 一份，由 `Project::switch_sequence` 复位（取舍见 `docs/adr/0009`）。

use super::*;
use crate::timeline::{Id, SequenceSpec, UndoHistory};
use gpui_kit::component::input::{Input, InputState};

/// 面板的四种形态。列表是常态，其余三种各自只占一屏。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SequenceManagerMode {
    List,
    New,
    Rename(Id),
    ConfirmDelete(Id),
}

pub(super) struct SequenceManagerDialog {
    pub(super) mode: SequenceManagerMode,
    /// 新建 / 改名共用一个输入框：同一时刻只有一种形态。
    pub(super) name: Entity<InputState>,
    pub(super) error: Option<String>,
}

impl PlayerApp {
    /// 打开序列面板。工程模式才有意义；再点一次入口就收起。
    pub(super) fn open_sequence_manager_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() {
            return;
        }
        if self.sequence_manager.is_some() {
            self.close_sequence_manager_dialog(cx);
            return;
        }
        // 模态互斥：叠在一起会抢同一份按键，也会让旧面板指向已经不存在的序列。
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
        self.project_confirm = None;
        self.show_settings = false;
        self.timeline_menu = None;
        self.menu = None;
        self.menu_bar = None;

        let name = cx.new(|cx| InputState::new(window, cx).placeholder("序列名字"));
        self.sequence_manager = Some(SequenceManagerDialog {
            mode: SequenceManagerMode::List,
            name,
            error: None,
        });
        cx.notify();
    }

    pub(super) fn close_sequence_manager_dialog(&mut self, cx: &mut Context<Self>) {
        if self.sequence_manager.take().is_some() {
            cx.notify();
        }
    }

    /// Esc 先退出子形态，回到列表再按一次才关面板；Enter 在输入形态里等于确定。
    pub(super) fn on_sequence_manager_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(dialog) = self.sequence_manager.as_ref() else {
            return false;
        };
        if event.keystroke.key == "escape" {
            if dialog.mode == SequenceManagerMode::List {
                self.close_sequence_manager_dialog(cx);
            } else if let Some(dialog) = self.sequence_manager.as_mut() {
                dialog.mode = SequenceManagerMode::List;
                dialog.error = None;
                cx.notify();
            }
            return true;
        }
        if event.keystroke.key == "enter"
            && matches!(
                dialog.mode,
                SequenceManagerMode::New | SequenceManagerMode::Rename(_)
            )
        {
            self.confirm_sequence_manager(cx);
            return true;
        }
        false
    }

    /// 切到新建 / 改名形态，并把输入框预填好。删除确认不需要输入框。
    fn begin_sequence_manager_mode(
        &mut self,
        mode: SequenceManagerMode,
        initial: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.sequence_manager.as_mut() else {
            return;
        };
        dialog.mode = mode;
        dialog.error = None;
        let state = dialog.name.clone();
        state.update(cx, |input, cx| {
            input.set_value(initial.to_string(), window, cx);
        });
        cx.notify();
    }

    /// 确定键：新建 / 改名 / 删除各做各的事，失败只留在面板里说明原因。
    pub(super) fn confirm_sequence_manager(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.sequence_manager.as_ref() else {
            return;
        };
        let mode = dialog.mode;
        let text = dialog.name.read(cx).value().to_string();
        let result = match mode {
            SequenceManagerMode::List => return,
            SequenceManagerMode::New => self.create_sequence(&text, cx),
            SequenceManagerMode::Rename(id) => self.rename_active_project_sequence(id, &text),
            SequenceManagerMode::ConfirmDelete(id) => self.delete_project_sequence(id, cx),
        };
        match result {
            Ok(message) => {
                self.sequence_manager = None;
                self.flash(message);
            }
            Err(error) => {
                if let Some(dialog) = self.sequence_manager.as_mut() {
                    dialog.error = Some(error);
                }
            }
        }
        cx.notify();
    }

    /// 新序列沿用当前序列的规格：改分辨率交给「序列规格…」，别在这里再摆一套表单。
    fn create_sequence(&mut self, name: &str, cx: &mut Context<Self>) -> Result<String, String> {
        // 借用限制在块里：切换序列要动的是整个 PlayerApp，不能攥着 &mut project 不放。
        let (id, label) = {
            let Some(project) = self.project.as_mut() else {
                return Err("工程已经关掉了".to_string());
            };
            let spec = SequenceSpec::new(
                project.sequence().width,
                project.sequence().height,
                project.sequence().fps,
            );
            let id = project.add_sequence(name, spec);
            let label = project
                .sequence_by_id(id)
                .map(|sequence| sequence.name.clone())
                .unwrap_or_default();
            (id, label)
        };
        self.switch_active_sequence(id, cx);
        Ok(format!("已新建并切到序列「{label}」"))
    }

    fn rename_active_project_sequence(&mut self, id: Id, name: &str) -> Result<String, String> {
        {
            let Some(project) = self.project.as_mut() else {
                return Err("工程已经关掉了".to_string());
            };
            project
                .rename_sequence(id, name)
                .map_err(|error| error.to_string())?;
        }
        self.note_edit();
        Ok(format!("序列已改名为「{}」", name.trim()))
    }

    fn delete_project_sequence(
        &mut self,
        id: Id,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        let (was_active, label, next) = {
            let Some(project) = self.project.as_mut() else {
                return Err("工程已经关掉了".to_string());
            };
            let was_active = project.active_id() == id;
            let label = project
                .sequence_by_id(id)
                .map(|sequence| sequence.name.clone())
                .unwrap_or_default();
            project
                .remove_sequence(id)
                .map_err(|error| error.to_string())?;
            (was_active, label, project.active_id())
        };
        if was_active {
            // 删的是当前序列：界面还挂着它的播放源与撤销栈，按新指针重来一遍。
            self.switch_active_sequence(next, cx);
        } else {
            self.note_edit();
        }
        Ok(format!("已删除序列「{label}」"))
    }

    /// 切换到另一条序列：先停播，再把所有指向旧序列的运行状态清干净。
    pub(super) fn switch_active_sequence(&mut self, id: Id, cx: &mut Context<Self>) {
        self.stop_timeline_playback(cx);
        self.stop_timeline_audio();
        self.timeline_preview = None;
        self.timeline_loaded_at = None;
        self.timeline_menu = None;
        self.timeline_dragging = false;
        self.timeline_drop = None;
        self.asset_drop = None;
        self.audition = None;
        self.source_asset = None;
        self.source_in = None;
        self.source_out = None;
        self.delete_track_confirm = None;
        self.sequence_dialog = None;
        // 导出面板记的是旧序列的 id：关掉它（顺手把改过的参数写回会话），别留着导错。
        self.close_export_dialog(cx);
        self.subtitle_dialog = None;

        let outcome = {
            let Some(project) = self.project.as_mut() else {
                return;
            };
            if let Err(error) = project.switch_sequence(id) {
                Err(error.to_string())
            } else {
                Ok((project.sequence().clone(), project.sequence().duration()))
            }
        };
        let (snapshot, duration) = match outcome {
            Ok(data) => data,
            Err(message) => {
                self.flash(message);
                return;
            }
        };
        // 撤销栈只服务当前序列：A 的历史不能退到 B 上。
        self.history = Some(UndoHistory::new(snapshot));
        self.timeline.center_on(0.0, duration);
        self.note_edit();
        self.preview_at_playhead(cx);
        cx.notify();
    }

    /// 面板：列表 / 输入 / 删前确认共用一张卡片。
    pub(super) fn render_sequence_manager_dialog(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.sequence_manager.as_ref()?;
        let project = self.project.as_ref()?;
        let active = project.active_id();

        let body = match dialog.mode {
            SequenceManagerMode::List => {
                let mut list = div().flex().flex_col().gap(px(6.0));
                for sequence in project.sequences() {
                    let id = sequence.id;
                    let current = id == active;
                    let detail = format!(
                        "{}×{} · {} 段",
                        sequence.width,
                        sequence.height,
                        sequence.clips().count()
                    );
                    let row = div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .py(px(6.0))
                        .px(px(8.0))
                        .rounded(px(8.0))
                        .bg(if current { ui::track() } else { surface() })
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(div().text_size(px(12.0)).text_color(text()).child(format!(
                                    "{}{}",
                                    sequence.name,
                                    if current { "（当前）" } else { "" }
                                )))
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(text_faint())
                                        .child(detail),
                                ),
                        )
                        .child(settings_button(
                            format!("seq-switch-{}", id.raw()),
                            "切换",
                            !current,
                            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                this.switch_active_sequence(id, cx);
                                this.flash("已切换序列");
                                cx.notify();
                            }),
                        ))
                        .child(settings_button(
                            format!("seq-rename-{}", id.raw()),
                            "改名",
                            true,
                            cx.listener(move |this, _: &ClickEvent, window, cx| {
                                let initial = this
                                    .project
                                    .as_ref()
                                    .and_then(|project| project.sequence_by_id(id))
                                    .map(|sequence| sequence.name.clone())
                                    .unwrap_or_default();
                                this.begin_sequence_manager_mode(
                                    SequenceManagerMode::Rename(id),
                                    &initial,
                                    window,
                                    cx,
                                );
                            }),
                        ))
                        .child(settings_button(
                            format!("seq-delete-{}", id.raw()),
                            "删除",
                            true,
                            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                if let Some(dialog) = this.sequence_manager.as_mut() {
                                    dialog.mode = SequenceManagerMode::ConfirmDelete(id);
                                    dialog.error = None;
                                }
                                cx.notify();
                            }),
                        ));
                    list = list.child(row);
                }
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(list)
                    .child(div().flex().justify_end().child(settings_button(
                        "seq-new",
                        "新建序列…",
                        true,
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.begin_sequence_manager_mode(
                                SequenceManagerMode::New,
                                "",
                                window,
                                cx,
                            );
                        }),
                    )))
                    .into_any_element()
            }
            SequenceManagerMode::New | SequenceManagerMode::Rename(_) => {
                let label = if dialog.mode == SequenceManagerMode::New {
                    "新序列沿用当前序列的分辨率与帧率，名字留空会自动编号。"
                } else {
                    "改个名字，片段与轨道都不动。"
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text_muted())
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .child(Input::new(&dialog.name).id("seq-manager-name").w_full()),
                    )
                    .child(self.manager_actions(cx))
                    .into_any_element()
            }
            SequenceManagerMode::ConfirmDelete(id) => {
                let label = project
                    .sequence_by_id(id)
                    .map(|sequence| sequence.name.clone())
                    .unwrap_or_default();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(text())
                            .child(format!("删除序列「{label}」？这条序列上的片段会一起删掉。")),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text_faint())
                            .child("删错了可以撤销工程文件，但当前这一步不会进撤销栈。"),
                    )
                    .child(self.manager_actions(cx))
                    .into_any_element()
            }
        };

        let mut card = div().flex().flex_col().gap(px(10.0)).child(body);
        if let Some(error) = &dialog.error {
            card = card.child(
                div()
                    .text_size(px(11.0))
                    .text_color(ui::danger())
                    .child(error.clone()),
            );
        }

        Some(modal_card(
            "sequence-manager",
            "序列",
            card.into_any_element(),
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.close_sequence_manager_dialog(cx);
            }),
        ))
    }

    /// 输入 / 删前确认底部的「确定 / 取消」。
    fn manager_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .justify_end()
            .gap(px(8.0))
            .child(settings_button(
                "seq-manager-cancel",
                "取消",
                true,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    if let Some(dialog) = this.sequence_manager.as_mut() {
                        dialog.mode = SequenceManagerMode::List;
                        dialog.error = None;
                    }
                    cx.notify();
                }),
            ))
            .child(primary_button(
                "seq-manager-confirm",
                "确定",
                true,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.confirm_sequence_manager(cx);
                }),
            ))
            .into_any_element()
    }
}
