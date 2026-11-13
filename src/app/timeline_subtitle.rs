//! 字幕编辑：条目文本 / 起止 / 样式的模态面板，以及字幕块上的小菜单。
//!
//! 字幕文本要输入框，而 `TimelineMenu` 必须保持 `Copy`，所以面板单独挂在
//! `PlayerApp` 上（与导出面板同一套做法）：打开时关掉别的模态，Esc 只收自己，
//! 文字输入交给获得焦点的 `InputState`。
//!
//! 「加字幕」不必先手工建字幕轨：确定时若没有可用的字幕轨会自动补一条，
//! 这样「新建工程 → 放素材 → 打字 → 导出」不会因为少点一步而卡住。

use super::*;
use crate::timeline::{
    COLORS, EditOp, FONTS, Id, MAX_OUTLINE, MAX_SIZE, MIN_SIZE, POSITIONS, SIZE_STEP, SubtitleItem,
    Tick, TrackKind,
};
use gpui_kit::component::input::{Input, InputState};

/// SRT / WebVTT 的导入与导出（面板之外的两个入口，菜单里的「导入 / 导出字幕文件…」）。
mod file;

/// 字幕面板的状态。`track` 为 `None` 表示「确定时自己挑 / 建一条字幕轨」。
pub(super) struct SubtitleDialog {
    track: Option<Id>,
    item: Option<Id>,
    draft: SubtitleItem,
    text: Entity<InputState>,
}

/// chip 的 id 必须是静态串：每帧换一个 id 会让 GPUI 认不出同一个元素。
const FONT_IDS: [&str; 3] = ["subtitle-font-0", "subtitle-font-1", "subtitle-font-2"];
const COLOR_IDS: [&str; 5] = [
    "subtitle-color-0",
    "subtitle-color-1",
    "subtitle-color-2",
    "subtitle-color-3",
    "subtitle-color-4",
];
const POSITION_IDS: [&str; 3] = [
    "subtitle-position-top",
    "subtitle-position-middle",
    "subtitle-position-bottom",
];
/// 新建字幕的默认时长：够看清一句话，也够在标尺上拖。
const DEFAULT_LENGTH: Tick = Tick::from_micros(3_000_000);

impl PlayerApp {
    /// 打开字幕面板。`item` 有值是编辑，`start` 是新建时的起点。
    pub(super) fn open_subtitle_dialog(
        &mut self,
        track: Option<Id>,
        item: Option<Id>,
        start: Tick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let draft = match item {
            Some(id) => match project.sequence().subtitle(id) {
                Some(existing) => existing.clone(),
                None => return,
            },
            None => {
                let start = start.max(Tick::ZERO);
                SubtitleItem::new(Id::default(), start, start + DEFAULT_LENGTH, "")
            }
        };
        if let Some(id) = track
            && project.sequence().track(id).is_some_and(|lane| lane.locked)
        {
            self.flash("轨道锁着，先解锁再加字幕");
            cx.notify();
            return;
        }

        // 模态互斥：叠在一起会互相挡住，也会抢同一份按键。
        self.close_remux_dialog(cx);
        self.close_metadata_dialog(cx);
        self.close_proxy_dialog(cx);
        self.close_scope_dialog(cx);
        self.close_scenes_dialog(cx);
        self.close_multicam_dialog(cx);
        self.close_concat_dialog(cx);
        self.close_clip_dialog(cx);
        self.close_playlist_import_dialog(cx);
        self.close_export_dialog(cx);
        self.close_sequence_dialog(cx);
        self.close_delete_track_confirm(cx);

        let text = cx.new(|cx| InputState::new(window, cx).placeholder("输入字幕文字"));
        text.update(cx, |state, cx| {
            state.set_value(draft.text.clone(), window, cx);
        });
        self.subtitle_dialog = Some(SubtitleDialog {
            track,
            item,
            draft,
            text,
        });
        self.timeline_menu = None;
        self.menu = None;
        self.menu_bar = None;
        cx.notify();
    }

    pub(super) fn close_subtitle_dialog(&mut self, cx: &mut Context<Self>) {
        if self.subtitle_dialog.take().is_some() {
            cx.notify();
        }
    }

    /// 面板开着时只接管 Esc；文字输入交给获得焦点的 Input。
    pub(super) fn on_subtitle_dialog_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.subtitle_dialog.is_none() {
            return false;
        }
        if event.keystroke.key == "escape" {
            self.close_subtitle_dialog(cx);
            return true;
        }
        false
    }

    /// 改一份草稿（起止或样式），不落撤销，确定时才算一步编辑。
    fn update_subtitle_draft(
        &mut self,
        update: impl FnOnce(&mut SubtitleItem),
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.subtitle_dialog.as_mut() else {
            return;
        };
        update(&mut dialog.draft);
        dialog.draft.style.normalize();
        cx.notify();
    }

    /// 起点 / 终点跟播放头走：面板里不用手输时间码，也不会出现倒着的区间。
    fn set_subtitle_bound(&mut self, at_end: bool, cx: &mut Context<Self>) {
        let at = self
            .project
            .as_ref()
            .map(|project| project.ui.playhead.max(Tick::ZERO))
            .unwrap_or(Tick::ZERO);
        self.update_subtitle_draft(
            |draft| {
                if at_end {
                    draft.end = at;
                    if draft.end <= draft.start {
                        draft.start = draft.end.saturating_sub(DEFAULT_LENGTH).max(Tick::ZERO);
                    }
                } else {
                    draft.start = at;
                    if draft.end <= draft.start {
                        draft.end = draft.start.saturating_add(DEFAULT_LENGTH);
                    }
                }
            },
            cx,
        );
    }

    /// 确定：新建走 `AddSubtitle`，编辑走 `SetSubtitle`，都落一步撤销。
    fn apply_subtitle_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.subtitle_dialog.as_ref() else {
            return;
        };
        let typed = dialog.text.read(cx).value().to_string();
        let mut item = dialog.draft.clone();
        item.text = typed;
        item.normalize();

        let Some(project) = self.project.as_ref() else {
            return;
        };
        let mut sequence = project.sequence().clone();
        let (op, message) = if let Some(id) = dialog.item {
            (EditOp::SetSubtitle { id, item }, "编辑字幕".to_string())
        } else {
            // 指定轨还用得了就用它；否则挑第一条没锁的字幕轨；都没有就建一条。
            // 建轨与加字幕合成一次编辑，取消时不会留下一条空轨。
            let track = dialog
                .track
                .filter(|id| {
                    sequence
                        .track(*id)
                        .is_some_and(|lane| lane.kind == TrackKind::Subtitle && !lane.locked)
                })
                .or_else(|| {
                    sequence
                        .tracks
                        .iter()
                        .find(|lane| lane.kind == TrackKind::Subtitle && !lane.locked)
                        .map(|lane| lane.id)
                })
                .unwrap_or_else(|| sequence.add_track(TrackKind::Subtitle));
            let id = sequence.alloc_id();
            let message = format!("添加字幕：{}", item.label());
            item.id = id;
            (EditOp::AddSubtitle { track, item }, message)
        };

        match crate::timeline::apply(&sequence, &op) {
            Ok(next) => {
                if self
                    .project
                    .as_ref()
                    .is_some_and(|project| *project.sequence() == next)
                {
                    self.flash("没有变化");
                } else {
                    self.commit_edit(next, message.clone());
                    self.flash(message);
                }
                self.close_subtitle_dialog(cx);
                self.poke();
            }
            Err(error) => self.flash(error.to_string()),
        }
        cx.notify();
    }

    fn delete_subtitle_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.subtitle_dialog.as_ref().and_then(|d| d.item) else {
            return;
        };
        let Some(project) = self.project.as_ref() else {
            return;
        };
        match crate::timeline::apply(project.sequence(), &EditOp::RemoveSubtitle { id }) {
            Ok(next) => {
                self.commit_edit(next, "删除字幕");
                self.flash("已删除字幕");
                self.close_subtitle_dialog(cx);
                self.poke();
            }
            Err(error) => self.flash(error.to_string()),
        }
        cx.notify();
    }

    /// 字幕块上右键：编辑 / 删除，不塞进片段菜单（字幕不是片段）。
    pub(super) fn open_subtitle_item_menu(
        &mut self,
        item: Id,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.timeline_menu = Some(TimelineMenu::Subtitle { item, position });
        cx.notify();
    }

    pub(super) fn render_subtitle_item_menu(
        &self,
        item: Id,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let project = self.project.as_ref()?;
        project.sequence().subtitle(item)?;
        let origin = super::timeline_menu::menu_origin(
            position,
            super::timeline_menu::menu_height(2, 0),
            window,
        );
        Some(
            div()
                .id("timeline-subtitle-menu")
                .absolute()
                .left(origin.x)
                .top(origin.y)
                .w(px(ITEM_MENU_WIDTH))
                .py(px(6.0))
                .rounded(px(10.0))
                .bg(surface())
                .border_1()
                .border_color(gpui_kit::rgb(0x2c2c34))
                .shadow_lg()
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_this, _: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                    }),
                )
                .child(item_menu_entry(
                    "timeline-subtitle-edit",
                    IconName::Pencil,
                    "编辑字幕…",
                    text(),
                    cx.listener(move |this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.timeline_menu = None;
                        this.open_subtitle_dialog(None, Some(item), Tick::ZERO, window, cx);
                    }),
                ))
                .child(item_menu_entry(
                    "timeline-subtitle-delete",
                    IconName::Trash,
                    "删除字幕",
                    ui::danger(),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.timeline_menu = None;
                        if let Some(project) = this.project.as_ref()
                            && let Ok(next) = crate::timeline::apply(
                                project.sequence(),
                                &EditOp::RemoveSubtitle { id: item },
                            )
                        {
                            this.commit_edit(next, "删除字幕");
                            this.flash("已删除字幕");
                            this.poke();
                        }
                        cx.notify();
                    }),
                ))
                .into_any_element(),
        )
    }

    /// 字幕面板：文本、起止、字体 / 字号 / 颜色 / 描边 / 位置。
    pub(super) fn render_subtitle_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.subtitle_dialog.as_ref()?;
        let draft = &dialog.draft;
        let style = &draft.style;

        let fonts = FONTS
            .iter()
            .enumerate()
            .map(|(index, font)| {
                chip(
                    FONT_IDS[index],
                    font,
                    style.font == *font,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.update_subtitle_draft(
                            |draft| draft.style.font = FONTS[index].to_string(),
                            cx,
                        );
                    }),
                )
            })
            .collect();
        let colors = COLORS
            .iter()
            .enumerate()
            .map(|(index, (rgb, label))| {
                let rgb = *rgb;
                chip(
                    COLOR_IDS[index],
                    label,
                    style.color == rgb,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.update_subtitle_draft(|draft| draft.style.color = rgb, cx);
                    }),
                )
            })
            .collect();
        let positions = POSITIONS
            .iter()
            .enumerate()
            .map(|(index, position)| {
                let position = *position;
                chip(
                    POSITION_IDS[index],
                    position.label(),
                    style.position == position,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.update_subtitle_draft(|draft| draft.style.position = position, cx);
                    }),
                )
            })
            .collect();

        let size_row = step_row(
            "字号",
            format!("{} px", style.size),
            "subtitle-size-minus",
            "subtitle-size-plus",
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.update_subtitle_draft(
                    |draft| {
                        draft.style.size = draft.style.size.saturating_sub(SIZE_STEP).max(MIN_SIZE)
                    },
                    cx,
                );
            }),
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.update_subtitle_draft(
                    |draft| {
                        draft.style.size = draft.style.size.saturating_add(SIZE_STEP).min(MAX_SIZE)
                    },
                    cx,
                );
            }),
        );
        let outline_row = step_row(
            "描边",
            format!("{} px", style.outline),
            "subtitle-outline-minus",
            "subtitle-outline-plus",
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.update_subtitle_draft(
                    |draft| draft.style.outline = draft.style.outline.saturating_sub(1),
                    cx,
                );
            }),
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.update_subtitle_draft(
                    |draft| {
                        draft.style.outline = draft.style.outline.saturating_add(1).min(MAX_OUTLINE)
                    },
                    cx,
                );
            }),
        );

        let mut actions = div()
            .flex()
            .justify_end()
            .gap(px(8.0))
            .child(settings_button(
                "subtitle-cancel",
                "取消",
                true,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.close_subtitle_dialog(cx);
                }),
            ));
        if dialog.item.is_some() {
            actions = actions.child(settings_button(
                "subtitle-delete",
                "删除",
                true,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.delete_subtitle_dialog(cx);
                }),
            ));
        }
        actions = actions.child(primary_button(
            "subtitle-apply",
            "确定",
            true,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.apply_subtitle_dialog(cx);
            }),
        ));

        let body = div()
            .flex()
            .flex_col()
            .gap(px(9.0))
            .child(settings_note("文字（换行请写 \\N）"))
            .child(Input::new(&dialog.text).id("subtitle-text").w_full())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(settings_note(format!(
                        "{:.2} – {:.2} 秒 · 时长 {:.2} 秒",
                        draft.start.secs(),
                        draft.end.secs(),
                        draft.duration().secs()
                    )))
                    .child(settings_button(
                        "subtitle-set-start",
                        "设为起点",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.set_subtitle_bound(false, cx);
                        }),
                    ))
                    .child(settings_button(
                        "subtitle-set-end",
                        "设为终点",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.set_subtitle_bound(true, cx);
                        }),
                    )),
            )
            .child(option_row("字体", fonts))
            .child(size_row)
            .child(option_row("颜色", colors))
            .child(outline_row)
            .child(option_row("位置", positions))
            .child(settings_note(style.summary()))
            .child(settings_note(
                "预览显示文字、颜色与位置；字体和描边只在导出时由 libass 排版。",
            ))
            .child(actions)
            .into_any_element();

        Some(modal_card(
            "subtitle-dialog",
            if dialog.item.is_some() {
                "编辑字幕"
            } else {
                "新建字幕"
            },
            body,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.close_subtitle_dialog(cx);
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
        .child(div().w(px(34.0)).flex_none().child(settings_note(label)))
        .children(chips)
        .into_any_element()
}

/// 一行「标签 + 值 + － / ＋」。
fn step_row(
    label: &'static str,
    value: String,
    minus_id: &'static str,
    plus_id: &'static str,
    minus: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    plus: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(div().w(px(34.0)).flex_none().child(settings_note(label)))
        .child(div().w(px(54.0)).child(settings_note(value)))
        .child(settings_button(minus_id, "－", true, minus))
        .child(settings_button(plus_id, "＋", true, plus))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{SubtitlePosition, SubtitleStyle};

    #[test]
    fn 面板的_chip_数组与选项一一对应() {
        assert_eq!(FONT_IDS.len(), FONTS.len());
        assert_eq!(COLOR_IDS.len(), COLORS.len());
        assert_eq!(POSITION_IDS.len(), POSITIONS.len());
        assert!(POSITIONS.contains(&SubtitlePosition::default()));
        assert_eq!(SubtitleStyle::default().font, FONTS[0]);
        assert_eq!(MIN_SIZE % SIZE_STEP, 0);
    }

    #[test]
    fn 新建字幕的默认区间是三秒且不落在负时间() {
        let start = Tick::from_micros(-1);
        let begin = start.max(Tick::ZERO);
        let item = SubtitleItem::new(Id::default(), begin, begin + DEFAULT_LENGTH, "");
        assert_eq!(item.duration(), DEFAULT_LENGTH);
        assert_eq!(item.start, Tick::ZERO);
    }
}
