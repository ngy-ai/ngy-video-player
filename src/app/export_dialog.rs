//! 导出面板：编码参数、预设选择与「导出…」入口。
//!
//! 点标题条或「文件」菜单里的导出，先在这里确认这套参数，再选文件名。参数本身
//! 不随工程存盘——它描述的是「这次怎么压」，不是「这条片子怎么剪」；要复用就
//! 存成预设，落在配置目录的 `render-presets.json`。
//!
//! 预设与当前参数「完全一致」才算用着它：改过任何一项之后选中态自然熄灭，
//! 队列项也据此记成「自定义」。

use super::*;
use crate::timeline::{
    EncodeSpeed, ExportSettings, Quality, RenderPresetStore, SubtitleMode, VideoCodec,
    normalize_preset_name,
};
use gpui_kit::component::input::{Input, InputState};

/// 面板的全部状态。参数改动只留在对话框里，关掉时写回 `PlayerApp::export_settings`。
pub(super) struct ExportDialog {
    pub(super) settings: ExportSettings,
    pub(super) presets: RenderPresetStore,
    pub(super) preset_name: Entity<InputState>,
}

/// 音频码率三档（kbps）。与 `ExportSettings::audio_kbps` 的夹取区间一致。
const AUDIO_RATES: [u32; 3] = [128, 192, 256];
const AUDIO_LABELS: [&str; 3] = ["128k", "192k", "256k"];
const AUDIO_IDS: [&str; 3] = ["export-audio-128", "export-audio-192", "export-audio-256"];

const CODEC_IDS: [&str; 2] = ["export-codec-h264", "export-codec-h265"];
const QUALITY_IDS: [&str; 3] = [
    "export-quality-high",
    "export-quality-standard",
    "export-quality-small",
];
const SPEED_IDS: [&str; 3] = [
    "export-speed-fast",
    "export-speed-medium",
    "export-speed-slow",
];
const SUBTITLE_IDS: [&str; 2] = ["export-subtitle-burn", "export-subtitle-soft"];

impl PlayerApp {
    /// 打开导出面板。再点一次同一个入口就收起。
    pub(super) fn open_export_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.is_none() {
            return;
        }
        if self.export_dialog.is_some() {
            self.close_export_dialog(cx);
            return;
        }
        // 计划先生成一次：缺素材、没有视频轨这类问题不该等用户填完参数才发现。
        let settings = self.export_settings.clone();
        if let Some(project) = self.project.as_ref()
            && let Err(error) =
                crate::timeline::plan_sequence_with(project, project.ui.range(), &settings)
        {
            self.flash(format!("无法导出：{error}"));
            cx.notify();
            return;
        }

        // 模态互斥：叠在一起会互相挡住，也会抢同一份按键。
        self.close_subtitle_dialog(cx);
        self.close_remux_dialog(cx);
        self.close_metadata_dialog(cx);
        self.close_proxy_dialog(cx);
        self.close_scope_dialog(cx);
        self.close_scenes_dialog(cx);
        self.close_multicam_dialog(cx);
        self.close_concat_dialog(cx);
        self.close_clip_dialog(cx);
        self.close_playlist_import_dialog(cx);

        let presets = RenderPresetStore::load();
        let preset_name = cx.new(|cx| InputState::new(window, cx).placeholder("预设名字"));
        self.export_dialog = Some(ExportDialog {
            settings,
            presets,
            preset_name,
        });
        self.menu = None;
        self.menu_bar = None;
        cx.notify();
    }

    /// 关闭面板，并把这次选的参数记在会话里：连着导几条时不必每次重选。
    pub(super) fn close_export_dialog(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.export_dialog.take() {
            self.export_settings = dialog.settings;
        }
        cx.notify();
    }

    /// 面板打开时只接管 Esc；文字输入交给获得焦点的 Input。
    pub(super) fn on_export_dialog_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.export_dialog.is_none() {
            return false;
        }
        if event.keystroke.key == "escape" {
            self.close_export_dialog(cx);
            return true;
        }
        false
    }

    /// 改一项导出参数。面板开着才生效。
    fn update_export_settings(
        &mut self,
        update: impl FnOnce(&mut ExportSettings),
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.export_dialog.as_mut() else {
            return;
        };
        update(&mut dialog.settings);
        cx.notify();
    }

    /// 套用一份预设。名字框一起填上：套用后微调再存回原名是最常见的下一步。
    fn apply_export_preset(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.export_dialog.as_mut() else {
            return;
        };
        let Some(preset) = dialog.presets.presets().get(index).cloned() else {
            return;
        };
        dialog.settings = preset.settings.clone();
        let input = dialog.preset_name.clone();
        input.update(cx, |state, cx| {
            state.set_value(preset.name.clone(), window, cx);
        });
        self.flash(format!("已套用导出预设「{}」", preset.name));
        cx.notify();
    }

    /// 把当前参数存成预设。名字为空时只提示，不写文件。
    fn save_export_preset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.export_dialog.as_ref() else {
            return;
        };
        let typed = dialog.preset_name.read(cx).value().to_string();
        let Some(name) = normalize_preset_name(&typed) else {
            self.flash("先给预设起个名字");
            cx.notify();
            return;
        };

        let (replaced, input) = {
            let Some(dialog) = self.export_dialog.as_mut() else {
                return;
            };
            let replaced = dialog.presets.upsert(&name, &dialog.settings);
            dialog.presets.save();
            (replaced, dialog.preset_name.clone())
        };
        input.update(cx, |state, cx| state.set_value("", window, cx));
        self.flash(match replaced {
            Some(true) => format!("已更新导出预设「{name}」"),
            _ => format!("已保存导出预设「{name}」"),
        });
        cx.notify();
    }

    /// 删掉一份预设。删的是存档，不可撤销——名字要在提示里说一遍。
    fn delete_export_preset(&mut self, index: usize, cx: &mut Context<Self>) {
        let removed = {
            let Some(dialog) = self.export_dialog.as_mut() else {
                return;
            };
            let name = dialog.presets.presets().get(index).map(|p| p.name.clone());
            if dialog.presets.remove(index) {
                dialog.presets.save();
                name
            } else {
                None
            }
        };
        if let Some(name) = removed {
            self.flash(format!("已删除导出预设「{name}」"));
        }
        cx.notify();
    }

    /// 面板上的「导出…」：收起面板，转到另存为与入队。
    pub(super) fn start_export_from_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.export_dialog.as_ref() else {
            return;
        };
        let settings = dialog.settings.clone();
        // 记下「这条用的是哪个预设」：参数被微调过就对不上任何一份，记成自定义。
        let preset = dialog.presets.find_match(&settings).map(str::to_string);
        self.export_dialog = None;
        self.export_settings = settings.clone();
        self.pick_export_path(settings, preset, window, cx);
    }

    /// 导出面板：参数 + 预设 + 一颗「导出…」。
    pub(super) fn render_export_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.export_dialog.as_ref()?;
        let current = dialog.presets.find_match(&dialog.settings);

        let mut preset_chips = div().flex().flex_wrap().items_center().gap(px(6.0));
        for (index, preset) in dialog.presets.presets().iter().enumerate() {
            preset_chips = preset_chips.child(preset_chip(
                index,
                &preset.name,
                current == Some(preset.name.as_str()),
                cx,
            ));
        }

        let codes = [VideoCodec::H264, VideoCodec::H265]
            .into_iter()
            .enumerate()
            .map(|(index, codec)| {
                chip(
                    CODEC_IDS[index],
                    codec.label(),
                    dialog.settings.codec == codec,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.update_export_settings(|settings| settings.codec = codec, cx);
                    }),
                )
            })
            .collect();

        let qualities = [Quality::High, Quality::Standard, Quality::Small]
            .into_iter()
            .enumerate()
            .map(|(index, quality)| {
                chip(
                    QUALITY_IDS[index],
                    quality.label(),
                    dialog.settings.quality == quality,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.update_export_settings(|settings| settings.quality = quality, cx);
                    }),
                )
            })
            .collect();

        let speeds = [EncodeSpeed::Fast, EncodeSpeed::Medium, EncodeSpeed::Slow]
            .into_iter()
            .enumerate()
            .map(|(index, speed)| {
                chip(
                    SPEED_IDS[index],
                    speed.label(),
                    dialog.settings.speed == speed,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.update_export_settings(|settings| settings.speed = speed, cx);
                    }),
                )
            })
            .collect();

        let audio = AUDIO_RATES
            .into_iter()
            .enumerate()
            .map(|(index, kbps)| {
                chip(
                    AUDIO_IDS[index],
                    AUDIO_LABELS[index],
                    dialog.settings.audio_bitrate == kbps,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.update_export_settings(|settings| settings.audio_bitrate = kbps, cx);
                    }),
                )
            })
            .collect();

        let subtitles = [SubtitleMode::Burn, SubtitleMode::Soft]
            .into_iter()
            .enumerate()
            .map(|(index, mode)| {
                chip(
                    SUBTITLE_IDS[index],
                    mode.label(),
                    dialog.settings.subtitle == mode,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.update_export_settings(|settings| settings.subtitle = mode, cx);
                    }),
                )
            })
            .collect();

        let preset_line = match current {
            Some(name) => format!("当前：{}（预设「{name}」）", dialog.settings.summary()),
            None => format!("当前：{}（自定义）", dialog.settings.summary()),
        };

        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(row_label("预设"))
            .when(!dialog.presets.is_empty(), |this| this.child(preset_chips))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div().flex_1().min_w(px(0.0)).child(
                            Input::new(&dialog.preset_name)
                                .id("export-preset-name")
                                .w_full(),
                        ),
                    )
                    .child(settings_button(
                        "export-preset-save",
                        "存为预设",
                        true,
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.save_export_preset(window, cx);
                        }),
                    )),
            )
            .child(option_row("编码", codes))
            .child(option_row("画质", qualities))
            .child(option_row("速度", speeds))
            .child(option_row("音频", audio))
            .child(option_row("字幕", subtitles))
            .child(settings_note(preset_line))
            .child(settings_note(
                "H.265 同画质体积更小、解码更吃 CPU；「速度」只影响编码耗时，不影响时长。",
            ))
            .child(settings_note(
                "烧录把字幕画进画面；软字幕另存一条字幕轨（mkv 存 ASS，mp4 存 mov_text，样式会少一些）。",
            ))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.0))
                    .child(settings_button(
                        "export-cancel",
                        "取消",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.close_export_dialog(cx);
                        }),
                    ))
                    .child(primary_button(
                        "export-start",
                        "导出…",
                        true,
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.start_export_from_dialog(window, cx);
                        }),
                    )),
            )
            .into_any_element();

        Some(modal_card(
            "export-dialog",
            "导出序列",
            body,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.close_export_dialog(cx);
            }),
        ))
    }
}

/// 一行「标签 + 一组 chip」。
fn option_row(label: &'static str, chips: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(div().w(px(34.0)).flex_none().child(row_label(label)))
        .children(chips)
        .into_any_element()
}

fn row_label(text: &'static str) -> AnyElement {
    div()
        .text_size(px(11.0))
        .text_color(text_faint())
        .child(text)
        .into_any_element()
}

/// 一份预设的小按钮：名字点一下套用，右边的 `×` 删除。
fn preset_chip(index: usize, name: &str, active: bool, cx: &mut Context<PlayerApp>) -> AnyElement {
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
                .id(SharedString::from(format!("export-preset-apply-{index}")))
                .max_w(px(140.0))
                .text_size(px(11.0))
                .text_color(ink)
                .cursor(CursorStyle::PointingHand)
                .child(ui::ellipsis(name.to_string()))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    cx.stop_propagation();
                    this.apply_export_preset(index, window, cx);
                })),
        )
        .child(
            div()
                .id(SharedString::from(format!("export-preset-delete-{index}")))
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
                    this.delete_export_preset(index, cx);
                })),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 音频档位与标签一一对应() {
        assert_eq!(AUDIO_RATES.len(), AUDIO_LABELS.len());
        assert_eq!(AUDIO_RATES.len(), AUDIO_IDS.len());
        // 默认值必须落在某一档上，否则面板一打开就是「一颗都没选中」。
        assert!(AUDIO_RATES.contains(&ExportSettings::default().audio_bitrate));
    }

    #[test]
    fn 每个选项都有静态_id() {
        assert_eq!(CODEC_IDS.len(), 2, "编码器有两档");
        assert_eq!(QUALITY_IDS.len(), 3, "画质有三档");
        assert_eq!(SPEED_IDS.len(), 3, "速度有三档");
        assert_eq!(SUBTITLE_IDS.len(), 2, "字幕有烧录 / 软字幕两档");
        assert_eq!(
            ExportSettings::default().subtitle,
            SubtitleMode::Burn,
            "默认烧录，老预设缺字段也落这一档"
        );
    }
}
