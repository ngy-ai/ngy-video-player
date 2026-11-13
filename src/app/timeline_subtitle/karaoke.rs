//! 字幕面板里的「逐字 / 卡拉 OK」时间轴：每个字一段，拖分界线改这一段的节拍。
//!
//! 拖的是**分界线**：相邻两个字共享一段总时长，把线往右挪就是前一字变长、后一字变短，
//! 整条字幕的起止点与其它字的节拍都不变。和速度曲线一样，拖动中只记落点、松手才写回
//! 草稿，确定时才算一步撤销。

use super::*;
use crate::app::timeline_tracks::TimelineGhost;

/// 拖的是第 `index` 个字与后一个字之间的分界线。
#[derive(Clone, Copy, Debug)]
pub(super) struct KaraokeDrag {
    pub index: usize,
}

/// 拖动中算出来的新权重（两个相邻字，和不变）。
#[derive(Clone, Copy, Debug)]
pub(super) struct KaraokeDrop {
    pub index: usize,
    pub left: u32,
    pub right: u32,
}

impl PlayerApp {
    /// 拖动中：按指针在条上的位置重算相邻两字的权重，只记落点不写草稿。
    fn drag_karaoke(
        &mut self,
        event: &gpui_kit::DragMoveEvent<KaraokeDrag>,
        cx: &mut Context<Self>,
    ) {
        let position = event.event.position;
        if !event.bounds.contains(&position) {
            return;
        }
        let index = event.drag(cx).index;
        let Some(dialog) = self.subtitle_dialog.as_ref() else {
            return;
        };
        let text = dialog.text.read(cx).value().to_string();
        let weights = dialog.draft.karaoke_weights(&text);
        if index + 1 >= weights.len() {
            return;
        }
        let total: f32 = weights.iter().map(|weight| *weight as f32).sum();
        let prefix: f32 = weights[..index].iter().map(|weight| *weight as f32).sum();
        let pair = weights[index].saturating_add(weights[index + 1]);
        let width = f32::from(event.bounds.size.width).max(1.0);
        let x = f32::from(position.x) - f32::from(event.bounds.origin.x);
        // 指针落在整条里的位置 → 落在这两个字组成的小段里的比例。
        let segment_start = prefix / total.max(1.0);
        let segment_width = pair as f32 / total.max(1.0);
        let fraction = if segment_width > 0.0 {
            (x / width - segment_start) / segment_width
        } else {
            0.5
        };
        // 两边都至少留 1：权重为 0 的字在 ASS 里会变成 `\kf0`，回到工程就被夹成 1。
        let Some((left, right)) = crate::timeline::move_karaoke_boundary(&weights, index, fraction)
        else {
            return;
        };
        if let Some(dialog) = self.subtitle_dialog.as_mut() {
            dialog.karaoke_drag = Some(KaraokeDrop { index, left, right });
        }
        cx.notify();
    }

    /// 松手：落点属于这次拖的那条线才写回草稿，避免上次拖到一半的旧落点串到新线。
    fn drop_karaoke(&mut self, drag: KaraokeDrag, cx: &mut Context<Self>) {
        let matched = self
            .subtitle_dialog
            .as_ref()
            .and_then(|dialog| dialog.karaoke_drag)
            .filter(|drop| drop.index == drag.index);
        if let Some(drop) = matched {
            self.sync_subtitle_text(cx);
            self.update_subtitle_draft(
                |draft| {
                    if drop.index + 1 < draft.karaoke.len() {
                        draft.karaoke[drop.index] = drop.left;
                        draft.karaoke[drop.index + 1] = drop.right;
                    }
                },
                cx,
            );
        }
        if let Some(dialog) = self.subtitle_dialog.as_mut() {
            dialog.karaoke_drag = None;
        }
        cx.notify();
    }
}

/// 一行开关 + 一条按权重分段的字条。文字取**输入框里当下那一份**，不是 draft.text
/// （后者要等确定才同步），所以刚打完字也能立刻按新字数调节拍。
pub(super) fn render_karaoke(
    dialog: &SubtitleDialog,
    typed: &str,
    cx: &mut Context<PlayerApp>,
) -> AnyElement {
    let enabled = dialog.draft.karaoke_enabled();
    let toggle = if enabled {
        settings_button(
            "subtitle-karaoke-off",
            "关掉逐字",
            true,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.sync_subtitle_text(cx);
                this.update_subtitle_draft(|draft| draft.set_karaoke(false), cx);
                if let Some(dialog) = this.subtitle_dialog.as_mut() {
                    dialog.karaoke_drag = None;
                }
            }),
        )
    } else {
        settings_button(
            "subtitle-karaoke-on",
            "开启逐字",
            true,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.sync_subtitle_text(cx);
                this.update_subtitle_draft(|draft| draft.set_karaoke(true), cx);
            }),
        )
    };

    let mut row = div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(option_row("逐字 / 卡拉 OK", vec![toggle]));
    if !enabled {
        return row
            .child(settings_note(
                "关着：整条字幕一起出现。打开后每个字按节拍依次高亮，可以拖动分界线调。",
            ))
            .into_any_element();
    }

    let chars: Vec<char> = typed
        .chars()
        .filter(|ch| *ch != '\n' && *ch != '\r')
        .collect();
    let mut weights = dialog.draft.karaoke_weights(typed);
    let preview = cx
        .has_active_drag()
        .then_some(dialog.karaoke_drag)
        .flatten()
        .filter(|drop| drop.index + 1 < weights.len());
    if let Some(drop) = preview {
        weights[drop.index] = drop.left;
        weights[drop.index + 1] = drop.right;
    }
    let total: f32 = weights
        .iter()
        .map(|weight| *weight as f32)
        .sum::<f32>()
        .max(1.0);

    let mut strip = div()
        .id("karaoke-strip")
        .relative()
        .w_full()
        .h(px(30.0))
        .border_1()
        .border_color(ui::track())
        .rounded(px(4.0))
        .bg(gpui_kit::rgba(0x0000_0033))
        .overflow_hidden();
    let mut cumulative = 0.0f32;
    for (index, weight) in weights.iter().enumerate() {
        let left = cumulative / total;
        let width = *weight as f32 / total;
        cumulative += *weight as f32;
        let active = preview.is_some_and(|drop| drop.index == index || drop.index + 1 == index);
        let label = chars
            .get(index)
            .map(|ch| ch.to_string())
            .unwrap_or_default();
        strip = strip.child(
            div()
                .absolute()
                .left(gpui_kit::relative(left))
                .top_0()
                .bottom_0()
                .w(gpui_kit::relative(width))
                .flex()
                .items_center()
                .justify_center()
                .border_r_1()
                .border_color(ui::track())
                .when(active, |this| this.bg(ui::accent()))
                .text_size(px(11.0))
                .text_color(if active { text() } else { text_muted() })
                .child(label),
        );
    }
    // 分界线比字窄得多，压在字上面，光标给出「能拖」的提示。
    cumulative = 0.0f32;
    for (index, weight) in weights
        .iter()
        .enumerate()
        .take(weights.len().saturating_sub(1))
    {
        cumulative += *weight as f32;
        let left = cumulative / total;
        strip = strip.child(
            div()
                .id(("karaoke-handle", index))
                .absolute()
                .left(gpui_kit::relative(left))
                .top_0()
                .bottom_0()
                .w(px(6.0))
                .ml(px(-3.0))
                .cursor(CursorStyle::PointingHand)
                .on_drag(KaraokeDrag { index }, move |_drag, _offset, _window, cx| {
                    cx.new(|_| TimelineGhost {
                        name: SharedString::from("逐字节拍"),
                    })
                }),
        );
    }
    strip = strip
        .on_drag_move(cx.listener(
            |this, event: &gpui_kit::DragMoveEvent<KaraokeDrag>, _window, cx| {
                this.drag_karaoke(event, cx);
            },
        ))
        .on_drop(cx.listener(|this, drag: &KaraokeDrag, _window, cx| {
            this.drop_karaoke(*drag, cx);
        }));

    row = row.child(strip).child(settings_note(format!(
        "共 {} 个字 · {:.2} 秒；左右拖动分界线改相邻两字的时长（总长不变）。\
             预览里唱过的字变黄，导出由 libass 按 ASS 的 `\\kf` 逐字填充。",
        chars.len(),
        dialog.draft.duration().secs()
    )));
    row.into_any_element()
}
