//! 序列规格对话框：新建工程时选一次，工程开着的时候也能改。
//!
//! 两个入口共用一套界面与校验，差别只在确定之后做什么——新建一个工程，
//! 还是改当前序列（改规格进撤销栈，和别的编辑一样能退回去）。
//!
//! 宽高既要能点预设、又要能直接输数字：两条路写同一个 `spec`，
//! 点预设时顺手把数字刷进输入框，用户就不必先清空再重打。

use super::*;
use crate::timeline::{Rational, SequenceSpec, rate_label};
use gpui_kit::Subscription;
use gpui_kit::component::input::{Input, InputState};

/// 对话框为什么弹出来。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SequencePurpose {
    /// 新建工程：确定之后才真的建。
    New,
    /// 改当前序列：确定后立刻生效，可以撤销。
    Edit,
}

impl SequencePurpose {
    fn title(self) -> &'static str {
        match self {
            SequencePurpose::New => "新建工程 · 序列规格",
            SequencePurpose::Edit => "序列规格",
        }
    }

    fn confirm(self) -> &'static str {
        match self {
            SequencePurpose::New => "建这个工程",
            SequencePurpose::Edit => "应用",
        }
    }
}

/// 对话框状态。关掉就丢掉，下次打开重新按当前序列填。
pub(super) struct SequenceDialog {
    pub(super) purpose: SequencePurpose,
    pub(super) spec: SequenceSpec,
    /// 自定义宽高。点预设时会把数字同步进来。
    pub(super) width_input: Entity<InputState>,
    pub(super) height_input: Entity<InputState>,
    /// 输入框的观察者：活着才收得到输入变化。
    _width_changes: Subscription,
    _height_changes: Subscription,
    /// 输入框里的内容解析不出来时的说明。
    pub(super) error: Option<String>,
}

impl PlayerApp {
    /// 菜单等入口请求打开对话框。手上没有 `window` 时挂起来，下一帧再开。
    pub(super) fn request_sequence_dialog(
        &mut self,
        purpose: SequencePurpose,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        match window {
            Some(window) => self.open_sequence_dialog(purpose, window, cx),
            None => self.pending_sequence_dialog = Some(purpose),
        }
    }

    /// 打开对话框。`purpose` 决定确定之后做什么。
    pub(super) fn open_sequence_dialog(
        &mut self,
        purpose: SequencePurpose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sequence_dialog.is_some() {
            return;
        }
        // 模态彼此互斥：开这一个先把别的收掉，不然两套键盘独占会打架。
        self.project_confirm = None;
        self.delete_track_confirm = None;
        self.show_settings = false;

        // 改现有序列时从当前值起步；新建时用默认值，素材库里有画面素材就按第一条来。
        let spec = match purpose {
            SequencePurpose::Edit => self
                .project
                .as_ref()
                .map(|project| {
                    SequenceSpec::new(
                        project.sequence().width,
                        project.sequence().height,
                        project.sequence().fps,
                    )
                })
                .unwrap_or_default(),
            SequencePurpose::New => self.first_asset_spec().unwrap_or_default(),
        };

        let width_text = spec.width.to_string();
        let height_text = spec.height.to_string();
        let width_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("宽")
                .default_value(width_text)
        });
        let height_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("高")
                .default_value(height_text)
        });

        let width_changes = cx.observe(&width_input, |this, state, cx| {
            let text = state.read(cx).value().to_string();
            this.on_sequence_size_input(true, &text, cx);
        });
        let height_changes = cx.observe(&height_input, |this, state, cx| {
            let text = state.read(cx).value().to_string();
            this.on_sequence_size_input(false, &text, cx);
        });

        self.sequence_dialog = Some(SequenceDialog {
            purpose,
            spec,
            width_input,
            height_input,
            _width_changes: width_changes,
            _height_changes: height_changes,
            error: None,
        });
        cx.notify();
    }

    /// 对话框打开时接管键盘：Esc 取消，Enter 确定。
    ///
    /// 返回 `true` 表示这一下已经处理掉了，别的快捷键不用再看。
    pub(super) fn on_sequence_dialog_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.sequence_dialog.is_none() {
            return false;
        }
        match event.keystroke.key.as_str() {
            "escape" => {
                self.close_sequence_dialog(cx);
                true
            }
            "enter" => {
                self.confirm_sequence_dialog(cx);
                true
            }
            _ => false,
        }
    }

    pub(super) fn close_sequence_dialog(&mut self, cx: &mut Context<Self>) {
        if self.sequence_dialog.take().is_none() {
            return;
        }
        cx.notify();
    }

    /// 素材库里第一条带画面的素材的规格。没有就返回 `None`（对话框退回默认值）。
    fn first_asset_spec(&self) -> Option<SequenceSpec> {
        let project = self.project.as_ref()?;
        project.assets.iter().find_map(|asset| {
            if asset.missing {
                return None;
            }
            asset.media.as_ref().and_then(SequenceSpec::from_media)
        })
    }

    /// 宽高输入框变化。解析不出数字就把错误留在对话框上，不动 `spec`。
    fn on_sequence_size_input(&mut self, is_width: bool, text: &str, cx: &mut Context<Self>) {
        let Some(dialog) = self.sequence_dialog.as_mut() else {
            return;
        };
        let trimmed = text.trim();
        match trimmed.parse::<u32>() {
            Ok(value) => {
                if is_width {
                    dialog.spec.width = value;
                } else {
                    dialog.spec.height = value;
                }
                dialog.error = None;
            }
            // 空串是「正在删」的中间态，别报错；真填了别的东西才说。
            Err(_) if trimmed.is_empty() => dialog.error = None,
            Err(_) => {
                dialog.error = Some(format!("「{trimmed}」不是有效的像素值，宽高只能填数字"));
            }
        }
        cx.notify();
    }

    /// 点预设分辨率：改 `spec`，同时把数字刷进输入框。
    ///
    /// `set_value` 不发 Change 事件，所以不会反过来再走一遍解析。
    fn set_sequence_resolution(
        &mut self,
        width: u32,
        height: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.sequence_dialog.as_mut() else {
            return;
        };
        dialog.spec.width = width;
        dialog.spec.height = height;
        dialog.error = None;
        let width_input = dialog.width_input.clone();
        let height_input = dialog.height_input.clone();
        width_input.update(cx, |state, cx| {
            state.set_value(width.to_string(), window, cx);
        });
        height_input.update(cx, |state, cx| {
            state.set_value(height.to_string(), window, cx);
        });
        cx.notify();
    }

    fn set_sequence_fps(&mut self, fps: Rational, cx: &mut Context<Self>) {
        let Some(dialog) = self.sequence_dialog.as_mut() else {
            return;
        };
        dialog.spec.fps = fps;
        dialog.error = None;
        cx.notify();
    }

    /// 按第一条素材填规格。没有可用素材时禁用，所以这里只管抄。
    fn use_first_asset_spec(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(spec) = self.first_asset_spec() else {
            return;
        };
        self.set_sequence_resolution(spec.width, spec.height, window, cx);
        self.set_sequence_fps(spec.fps, cx);
    }

    /// 确定：校验通过才生效。
    pub(super) fn confirm_sequence_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.sequence_dialog.as_ref() else {
            return;
        };
        let spec = dialog.spec;
        let purpose = dialog.purpose;
        if let Err(error) = spec.validate() {
            if let Some(dialog) = self.sequence_dialog.as_mut() {
                dialog.error = Some(error.to_string());
            }
            cx.notify();
            return;
        }
        self.sequence_dialog = None;
        match purpose {
            SequencePurpose::New => self.new_project(spec, cx),
            SequencePurpose::Edit => self.apply_sequence_spec(spec, cx),
        }
    }

    /// 把规格改到当前序列上。只改像素与帧对齐，片段的时间一个都不动。
    pub(super) fn apply_sequence_spec(&mut self, spec: SequenceSpec, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let current = SequenceSpec::new(
            project.sequence().width,
            project.sequence().height,
            project.sequence().fps,
        );
        if current == spec {
            self.flash("序列规格没有变化");
            cx.notify();
            return;
        }
        let mut sequence = project.sequence().clone();
        sequence.width = spec.width;
        sequence.height = spec.height;
        sequence.fps = spec.fps;
        // 走 `commit_edit`：和别的编辑一样进撤销栈，改错了能退回去。
        self.commit_edit(sequence, format!("序列规格改为 {}", spec.label()));
        self.flash(format!("序列规格已改为 {}", spec.label()));
        self.poke();
        cx.notify();
    }

    pub(super) fn render_sequence_dialog(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.sequence_dialog.as_ref()?;
        let spec = dialog.spec;
        let purpose = dialog.purpose;
        let error = dialog.error.clone();
        let width_input = dialog.width_input.clone();
        let height_input = dialog.height_input.clone();
        let can_use_asset = self.first_asset_spec().is_some();

        let resolutions = SequenceSpec::PRESETS
            .iter()
            .map(|(width, height, name)| {
                let width = *width;
                let height = *height;
                let active = spec.width == width && spec.height == height;
                spec_chip(
                    format!("seq-preset-{width}x{height}"),
                    format!("{name}  {width}×{height}"),
                    active,
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.set_sequence_resolution(width, height, window, cx);
                    }),
                )
            })
            .collect::<Vec<_>>();

        let rates = SequenceSpec::RATES
            .iter()
            .map(|fps| {
                let fps = *fps;
                spec_chip(
                    format!("seq-fps-{}-{}", fps.num(), fps.den()),
                    rate_label(fps),
                    spec.fps == fps,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.set_sequence_fps(fps, cx);
                    }),
                )
            })
            .collect::<Vec<_>>();

        let body = div()
            .flex()
            .flex_col()
            .gap(px(14.0))
            .w(px(460.0))
            .child(section_label("分辨率"))
            .child(div().flex().flex_wrap().gap(px(6.0)).children(resolutions))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text_faint())
                            .child("宽 × 高"),
                    )
                    .child(
                        div()
                            .w(px(96.0))
                            .child(Input::new(&width_input).id("seq-width").w_full()),
                    )
                    .child(div().text_color(text_faint()).child("×"))
                    .child(
                        div()
                            .w(px(96.0))
                            .child(Input::new(&height_input).id("seq-height").w_full()),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(text_faint())
                            .child("像素，必须为偶数"),
                    ),
            )
            .child(section_label("帧率"))
            .child(div().flex().flex_wrap().gap(px(6.0)).children(rates))
            .when_some(error, |this, error| {
                this.child(
                    div()
                        .text_size(px(11.0))
                        .text_color(ui::danger())
                        .whitespace_normal()
                        .child(error),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(settings_button(
                                "seq-use-asset",
                                "按第一个素材创建",
                                can_use_asset,
                                cx.listener(|this, _: &ClickEvent, window, cx| {
                                    this.use_first_asset_spec(window, cx);
                                }),
                            ))
                            .when(!can_use_asset, |this| {
                                this.child(
                                    div()
                                        .text_size(px(10.0))
                                        .text_color(text_faint())
                                        .child("素材库里还没有带画面的素材"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(settings_button(
                                "seq-cancel",
                                "取消",
                                true,
                                cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    this.close_sequence_dialog(cx);
                                }),
                            ))
                            .child(settings_button(
                                "seq-confirm",
                                purpose.confirm(),
                                true,
                                cx.listener(|this, _: &ClickEvent, _window, cx| {
                                    this.confirm_sequence_dialog(cx);
                                }),
                            )),
                    ),
            );

        Some(modal_card(
            "sequence-dialog",
            purpose.title(),
            body.into_any_element(),
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.close_sequence_dialog(cx);
            }),
        ))
    }
}

/// 一节的小标题。
fn section_label(title: &'static str) -> AnyElement {
    div()
        .text_size(px(11.0))
        .text_color(text_muted())
        .child(title)
        .into_any_element()
}

/// 可点的规格选项。选中时用主题色描边。
fn spec_chip(
    id: String,
    label: String,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .h(px(26.0))
        .px(px(10.0))
        .flex_none()
        .rounded(px(6.0))
        .border_1()
        // 选中态用主题色，未选中用分隔线色。分开写两遍而不是 `if`：
        // 两侧的颜色类型不同（`Rgba` / `Hsla`），同一个 `if` 表达式里合不到一起。
        .when(active, |this| this.border_color(ui::accent()))
        .when(!active, |this| this.border_color(ui::track()))
        .when(active, |this| this.bg(gpui_kit::rgba(0x5aa9ff26)))
        .when(!active, |this| this.bg(ui::surface_hover()))
        .text_size(px(12.0))
        .when(active, |this| this.text_color(ui::accent()))
        .when(!active, |this| this.text_color(text()))
        .cursor(CursorStyle::PointingHand)
        .hover(|style| style.bg(ui::surface_active()))
        .on_click(
            move |event: &ClickEvent, window: &mut Window, cx: &mut App| {
                // 弹窗浮在画面上：不截断的话同一次点击会漏到视频区。
                cx.stop_propagation();
                on_click(event, window, cx);
            },
        )
        .child(label)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 两种用途的标题与确定文案不同() {
        assert_ne!(SequencePurpose::New.title(), SequencePurpose::Edit.title());
        assert_eq!(SequencePurpose::New.confirm(), "建这个工程");
        assert_eq!(SequencePurpose::Edit.confirm(), "应用");
    }
}
