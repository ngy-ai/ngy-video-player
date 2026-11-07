//! 片段效果栈面板：加、开关、换档、调序、删。
//!
//! 与转场面板同一个外壳：贴在鼠标位置，点面板外面收掉。每次改动各落一步撤销，
//! 所以开关一下也算一步——试效果时来回按 Ctrl+Z 就能回到原样。
//!
//! 面板保持打开：调一条效果通常要连着点几下（加一个、换个档、挪个位置），
//! 点一下就得重新右键一次太烦。

use super::*;
use crate::timeline::{EffectKind, Id, Knob};

/// 面板上下文：作用在哪个片段、贴在哪里。
#[derive(Clone, Copy, Debug)]
pub(super) struct EffectsMenu {
    clip: Id,
    position: Point<Pixels>,
}

const PANEL_WIDTH: f32 = 300.0;
const ROW_HEIGHT: f32 = 26.0;
/// 标题、说明、分隔线与底部「加效果」两行占的高度。
const CHROME_HEIGHT: f32 = 112.0;
/// 水印字号的三档（输出画面像素）。
const SIZE_CHOICES: [u32; 3] = [24, 28, 36];

impl PlayerApp {
    /// 打开片段的效果面板。片段已经不在时什么都不做。
    pub(super) fn open_effects_menu(
        &mut self,
        clip: Id,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        if project.sequence.clip(clip).is_none() {
            return;
        }
        self.timeline_menu = Some(TimelineMenu::Effects(EffectsMenu { clip, position }));
        cx.notify();
    }

    /// 效果栈的改动统一走这里：先拷一份序列再改，成功才落一步撤销。
    ///
    /// `change` 返回提示语；返回 `None` 表示没有改动（比如挪到边界外），
    /// 这时什么都不做，免得撤销栈里多一步空操作。
    fn edit_effects(
        &mut self,
        clip: Id,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut crate::timeline::Sequence) -> Option<String>,
    ) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let locked = project
            .sequence
            .clip_track(clip)
            .and_then(|track| project.sequence.track(track))
            .is_some_and(|track| track.locked);
        if locked {
            self.flash("轨道锁着，先解锁再改效果");
            cx.notify();
            return;
        }
        let mut sequence = project.sequence.clone();
        let Some(message) = change(&mut sequence) else {
            return;
        };
        self.commit_edit(sequence, message.clone());
        self.flash(message);
        self.poke();
        cx.notify();
    }

    /// 效果面板：每行一条效果，底部一行「加效果」。
    pub(super) fn render_effects_menu(
        &self,
        menu: EffectsMenu,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let project = self.project.as_ref()?;
        // 片段被删掉时菜单跟着收掉，不留一张指向不存在片段的壳。
        let clip = project.sequence.clip(menu.clip)?;
        let track = project
            .sequence
            .clip_track(menu.clip)
            .and_then(|track| project.sequence.track(track));
        let locked = track.is_some_and(|track| track.locked);
        let title = format!("效果 · {}", clip_title(project, clip));
        let effects = clip.effects.clone();
        let fps = crate::timeline::sequence_fps(&project.sequence);

        let height = CHROME_HEIGHT + effects.len() as f32 * ROW_HEIGHT;
        let origin = super::timeline_menu::menu_origin(menu.position, height, window);
        let clip_id = menu.clip;

        let mut rows = div().flex().flex_col().gap(px(2.0));
        if effects.is_empty() {
            rows = rows.child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_faint())
                    .child("还没有效果：点下面的类型加一个"),
            );
        }
        for (index, effect) in effects.iter().enumerate() {
            let effect_id = effect.id;
            let enabled = effect.enabled;
            let kind = effect.kind;

            let dot = div()
                .id(("effect-dot", index))
                .w(px(10.0))
                .h(px(10.0))
                .rounded(px(3.0))
                .bg(if enabled { ui::accent() } else { ui::track() })
                .cursor(CursorStyle::PointingHand)
                .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.edit_effects(clip_id, cx, |sequence| {
                        let now = crate::timeline::toggle_effect(sequence, clip_id, effect_id)?;
                        Some(format!(
                            "{}{}",
                            kind.label(),
                            if now { "：开" } else { "：关" }
                        ))
                    });
                }));

            let name_color = if enabled { text() } else { text_faint() };
            let mut row = div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .h(px(ROW_HEIGHT))
                .child(dot)
                .child(
                    div()
                        .w(px(52.0))
                        .text_size(px(12.0))
                        .text_color(name_color)
                        .child(kind.label()),
                );

            // 参数：强度 / 倍数点一下换一档，水印是位置与字号。
            match kind.knob() {
                Knob::None => {}
                Knob::Strength | Knob::Factor => {
                    let label = if kind.knob() == Knob::Factor {
                        format!("{}×", effect.params.factor)
                    } else {
                        effect.params.strength_label().to_string()
                    };
                    row = row.child(
                        div()
                            .id(("effect-knob", index))
                            .px(px(8.0))
                            .py(px(2.0))
                            .rounded(px(5.0))
                            .text_size(px(11.0))
                            .bg(ui::track())
                            .text_color(text_muted())
                            .cursor(CursorStyle::PointingHand)
                            .child(label)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                this.edit_effects(clip_id, cx, |sequence| {
                                    crate::timeline::cycle_knob(sequence, clip_id, effect_id);
                                    let now = sequence
                                        .clip(clip_id)
                                        .and_then(|clip| {
                                            clip.effects.iter().find(|item| item.id == effect_id)
                                        })
                                        .map(|item| item.describe())
                                        .unwrap_or_else(|| kind.label().to_string());
                                    Some(now)
                                });
                            })),
                    );
                }
                Knob::Text => {
                    let position = effect.params.position;
                    row = row.child(
                        div()
                            .id(("effect-position", index))
                            .px(px(8.0))
                            .py(px(2.0))
                            .rounded(px(5.0))
                            .text_size(px(11.0))
                            .bg(ui::track())
                            .text_color(text_muted())
                            .cursor(CursorStyle::PointingHand)
                            .child(position.label())
                            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                this.edit_effects(clip_id, cx, |sequence| {
                                    let item = sequence
                                        .clip_mut(clip_id)?
                                        .effects
                                        .iter_mut()
                                        .find(|item| item.id == effect_id)?;
                                    let next = next_option(
                                        &crate::clip::WatermarkPosition::ALL,
                                        item.params.position,
                                    );
                                    item.params.position = next;
                                    Some(format!("水印位置：{}", next.label()))
                                });
                            })),
                    );
                    let size = effect.params.size;
                    row = row.child(
                        div()
                            .id(("effect-size", index))
                            .px(px(8.0))
                            .py(px(2.0))
                            .rounded(px(5.0))
                            .text_size(px(11.0))
                            .bg(ui::track())
                            .text_color(text_muted())
                            .cursor(CursorStyle::PointingHand)
                            .child(format!("{size}px"))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                this.edit_effects(clip_id, cx, |sequence| {
                                    let item = sequence
                                        .clip_mut(clip_id)?
                                        .effects
                                        .iter_mut()
                                        .find(|item| item.id == effect_id)?;
                                    let next = next_option(&SIZE_CHOICES, item.params.size);
                                    item.params.size = next;
                                    Some(format!("水印字号：{next}px"))
                                });
                            })),
                    );
                }
            }

            row = row.child(div().flex_1());
            // 顺序决定链上的先后：上移 / 下移各一颗。
            for (arrow, up) in [("↑", true), ("↓", false)] {
                row = row.child(
                    div()
                        .id(if up {
                            ("effect-up", index)
                        } else {
                            ("effect-down", index)
                        })
                        .w(px(16.0))
                        .text_center()
                        .text_size(px(12.0))
                        .text_color(text_muted())
                        .cursor(CursorStyle::PointingHand)
                        .child(arrow)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.edit_effects(clip_id, cx, |sequence| {
                                crate::timeline::move_effect(sequence, clip_id, effect_id, up).then(
                                    || {
                                        format!(
                                            "{}：{}移一位",
                                            kind.label(),
                                            if up { "上" } else { "下" }
                                        )
                                    },
                                )
                            });
                        })),
                );
            }
            row = row.child(
                div()
                    .id(("effect-remove", index))
                    .w(px(16.0))
                    .text_center()
                    .text_size(px(12.0))
                    .text_color(text_muted())
                    .cursor(CursorStyle::PointingHand)
                    .child("×")
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.edit_effects(clip_id, cx, |sequence| {
                            crate::timeline::remove_effect(sequence, clip_id, effect_id)
                                .then(|| format!("已删掉{}", kind.label()))
                        });
                    })),
            );
            rows = rows.child(row);
        }

        let mut kinds = div().flex().flex_wrap().gap(px(4.0));
        for (index, option) in EffectKind::ALL.into_iter().enumerate() {
            kinds = kinds.child(chip(
                EFFECT_CHIP_IDS[index],
                option.label(),
                false,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.edit_effects(clip_id, cx, |sequence| {
                        crate::timeline::add_effect(sequence, clip_id, option)
                            .map(|_| format!("加上{}", option.label()))
                    });
                }),
            ));
        }

        let footer = if locked {
            "轨道已锁定，先解锁再改".to_string()
        } else if effects
            .iter()
            .any(|effect| effect.kind == EffectKind::Interpolate)
        {
            format!("插帧很慢，导出 {:.0} fps 的成片会明显变久", fps * 2.0)
        } else {
            "按顺序生效，导出时逐条拼进滤镜链".to_string()
        };

        Some(
            div()
                .id("timeline-effects-menu")
                .absolute()
                .left(origin.x)
                .top(origin.y)
                .w(px(PANEL_WIDTH))
                .py(px(8.0))
                .px(px(10.0))
                .rounded(px(10.0))
                .bg(surface())
                .border_1()
                .border_color(gpui_kit::rgb(0x2c2c34))
                .shadow_lg()
                .flex()
                .flex_col()
                .gap(px(6.0))
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
                        .text_size(px(11.0))
                        .text_color(text_muted())
                        .child(title),
                )
                .child(rows)
                .child(div().mt(px(2.0)).h(px(1.0)).bg(gpui_kit::rgb(0x2c2c34)))
                .child(kinds)
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(text_faint())
                        .child(footer),
                )
                .into_any_element(),
        )
    }
}

/// 片段在面板标题里的名字：自己起过名就用名字，否则退回素材文件名。
fn clip_title(project: &crate::timeline::Project, clip: &crate::timeline::ClipItem) -> String {
    if !clip.name.trim().is_empty() {
        return clip.name.clone();
    }
    project
        .assets
        .iter()
        .find(|asset| asset.id == clip.asset_id)
        .and_then(|asset| asset.path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "片段".to_string())
}

/// 取列表里的下一个值，到末尾回到开头。
fn next_option<T: Copy + PartialEq>(choices: &[T], current: T) -> T {
    let index = choices
        .iter()
        .position(|choice| *choice == current)
        .unwrap_or(0);
    choices[(index + 1) % choices.len()]
}

/// 效果类型 chip 的 id：GPUI 要求 id 是静态生命周期。
const EFFECT_CHIP_IDS: [&str; 6] = [
    "effect-add-deinterlace",
    "effect-add-denoise",
    "effect-add-sharpen",
    "effect-add-stabilize",
    "effect-add-interpolate",
    "effect-add-watermark",
];
