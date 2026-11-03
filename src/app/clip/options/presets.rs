//! 导出预设：把当前这套参数起个名字存下来，下次一键套用。
//!
//! 参数默认只对一次导出有效，预设是唯一跨会话保留的那一份，所以摆在所有选项最
//! 上面——先挑一个现成的、再微调，是最常用的顺序。名字点一下套用，`×` 删除。
//!
//! 水印文字存在输入框里（自绘的数字框接不住中文输入法），所以套用预设时要把它
//! 一起换掉；保存时也要先从输入框取值，否则存下来的是上一个预设的文字。

use super::super::widgets::{note, section_title};
use super::super::*;
use crate::clip::{ExportPreset, normalize_name};
use gpui_kit::component::input::Input;

impl PlayerApp {
    /// 预设区：已存的预设 + 保存入口。
    pub(super) fn render_clip_presets(
        &self,
        dialog: &ClipDialog,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut list = div().flex().flex_wrap().items_center().gap(px(6.0));
        for (index, preset) in dialog.presets.presets().iter().enumerate() {
            // 当前参数与某份预设完全一致时高亮它：改过之后自然熄灭，
            // 用户不用去猜「现在用的是哪个预设」。
            list = list.child(preset_chip(
                index,
                preset,
                dialog.options == preset.options,
                cx,
            ));
        }

        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(section_title("预设"))
            .when(!dialog.presets.is_empty(), |this| this.child(list))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div().flex_1().min_w(px(0.0)).child(
                            Input::new(&dialog.preset_name)
                                .id("clip-preset-name")
                                .w_full(),
                        ),
                    )
                    .child(settings_button(
                        "clip-preset-save",
                        "存为预设",
                        true,
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.save_clip_preset(window, cx);
                        }),
                    )),
            )
            .child(note(
                "把当前这套参数存下来，下次打开面板点名字就能套用；同名会覆盖。",
            ))
            .into_any_element()
    }

    /// 套用一份预设。水印文字连同输入框一起换掉。
    fn apply_clip_preset(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.clip.as_mut() else {
            return;
        };
        let Some(preset) = dialog.presets.presets().get(index).cloned() else {
            return;
        };
        dialog.options = preset.options.clone();
        let text = dialog.options.watermark.text.clone();
        dialog
            .watermark_text
            .update(cx, |state, cx| state.set_value(text, window, cx));
        // 预设里不含「这次要做什么」：那是按当前文件的时间轴选的，不该被套用。
        self.flash(format!("已套用预设「{}」", preset.name));
        cx.notify();
    }

    /// 把当前参数存成预设。名字为空时只提示，不写文件。
    fn save_clip_preset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.clip.as_mut() else {
            return;
        };
        // 水印文字以输入框为准：参数里那份可能是打开面板时留下的旧值。
        let mut options = dialog.options.clone();
        options.watermark.text = dialog.watermark_text.read(cx).value().trim().to_string();

        let typed = dialog.preset_name.read(cx).value().to_string();
        let Some(name) = normalize_name(&typed) else {
            self.flash("先给预设起个名字");
            cx.notify();
            return;
        };
        let replaced = dialog.presets.upsert(&name, &options);
        dialog.presets.save();
        dialog
            .preset_name
            .update(cx, |state, cx| state.set_value("", window, cx));
        self.flash(match replaced {
            Some(true) => format!("已更新预设「{name}」"),
            _ => format!("已保存预设「{name}」"),
        });
        cx.notify();
    }

    /// 删掉一份预设。删的是存档，不可撤销——所以名字要在提示里说一遍。
    fn delete_clip_preset(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(dialog) = self.clip.as_mut() else {
            return;
        };
        let name = dialog.presets.presets().get(index).map(|p| p.name.clone());
        if dialog.presets.remove(index) {
            dialog.presets.save();
            if let Some(name) = name {
                self.flash(format!("已删除预设「{name}」"));
            }
        }
        cx.notify();
    }
}

/// 一份预设的小按钮：名字点一下套用，右边的 `×` 删除。
fn preset_chip(
    index: usize,
    preset: &ExportPreset,
    active: bool,
    cx: &mut Context<PlayerApp>,
) -> AnyElement {
    let ink: Hsla = if active {
        gpui_kit::rgb(0x111114).into()
    } else {
        text_muted()
    };
    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .h(px(26.0))
        .pl(px(9.0))
        .pr(px(5.0))
        .rounded(px(6.0))
        .when(active, |this| this.bg(ui::accent()))
        .when(!active, |this| {
            this.bg(ui::surface_hover())
                .hover(|style| style.bg(ui::surface_active()))
        })
        .child(
            div()
                .id(SharedString::from(format!("clip-preset-apply-{index}")))
                .max_w(px(140.0))
                .text_size(px(11.0))
                .text_color(ink)
                .cursor(CursorStyle::PointingHand)
                .child(ui::ellipsis(preset.name.clone()))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.apply_clip_preset(index, window, cx);
                })),
        )
        .child(
            div()
                .id(SharedString::from(format!("clip-preset-delete-{index}")))
                .flex()
                .items_center()
                .justify_center()
                .size(px(16.0))
                .rounded(px(4.0))
                .text_size(px(11.0))
                .text_color(ink)
                .cursor(CursorStyle::PointingHand)
                .hover(|style| style.bg(ui::surface_hover()).text_color(ui::danger()))
                .child("×")
                .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                    this.delete_clip_preset(index, cx);
                })),
        )
        .into_any_element()
}
