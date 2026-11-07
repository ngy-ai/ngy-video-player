//! 片段变速与片尾定格面板：倍率档位、定格时长，各一行互斥 chip。
//!
//! 改的是片段在时间线上的长度，后面的片段由 `timeline::set_speed` / `set_hold`
//! 整体让位（波纹）。面板每次改动走一个 `EditOp`，锁轨校验与撤销都和白捡一样。
//!
//! 面板保持打开：调倍率常常要连试几档，点一下就得重新右键一次太烦。

use super::*;
use crate::timeline::{ClipItem, EditOp, Id, Tick};

/// 面板上下文：作用在哪个片段、贴在哪里。
#[derive(Clone, Copy, Debug)]
pub(super) struct SpeedMenu {
    clip: Id,
    position: Point<Pixels>,
}

const PANEL_WIDTH: f32 = 300.0;
/// 标题 + 两行 chip + 一行说明，只用来判断在鼠标下方放不放得下。
const PANEL_HEIGHT: f32 = 132.0;

/// 定格档位：0 是「取消」，其余是停在最后一帧的秒数。
const HOLD_CHOICES: [Tick; 5] = [
    Tick::ZERO,
    Tick::from_millis(500),
    Tick::from_millis(1000),
    Tick::from_millis(2000),
    Tick::from_millis(3000),
];
const HOLD_LABELS: [&str; 5] = ["不定格", "0.5 秒", "1 秒", "2 秒", "3 秒"];

impl PlayerApp {
    /// 打开片段的变速面板。片段已经不在时什么都不做。
    pub(super) fn open_speed_menu(
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
        self.timeline_menu = Some(TimelineMenu::Speed(SpeedMenu { clip, position }));
        cx.notify();
    }

    /// 变速与定格共用一个出口：`EditOp` 自带锁轨与未知片段校验。
    ///
    /// 新序列和原来一模一样时不落撤销——界面点的是当前档位，不该多出一步空操作。
    fn apply_speed_op(&mut self, op: EditOp, message: String, cx: &mut Context<Self>) {
        let result = self
            .project
            .as_ref()
            .map(|project| crate::timeline::apply(&project.sequence, &op));
        match result {
            Some(Ok(next)) => {
                if self
                    .project
                    .as_ref()
                    .is_some_and(|project| project.sequence == next)
                {
                    self.flash("没有变化");
                    cx.notify();
                    return;
                }
                self.commit_edit(next, message.clone());
                self.flash(message);
                self.poke();
                cx.notify();
            }
            Some(Err(error)) => {
                self.flash(error.to_string());
                cx.notify();
            }
            None => {}
        }
    }

    /// 变速 / 定格面板。
    pub(super) fn render_speed_menu(
        &self,
        menu: SpeedMenu,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let project = self.project.as_ref()?;
        let clip = project.sequence.clip(menu.clip)?;
        let locked = project
            .sequence
            .clip_track(menu.clip)
            .and_then(|track| project.sequence.track(track))
            .is_some_and(|track| track.locked);
        let rate = clip.rate();
        let hold = clip.hold.max(Tick::ZERO);
        let clip_id = menu.clip;
        let duration = clip.duration();
        let span = clip.span();
        let title = format!(
            "变速 / 定格 · {}",
            super::timeline_effects::clip_title(project, clip)
        );
        let origin = super::timeline_menu::menu_origin(menu.position, PANEL_HEIGHT, window);

        let mut speeds = div().flex().flex_wrap().gap(px(4.0));
        for (index, option) in ClipItem::SPEEDS.into_iter().enumerate() {
            speeds = speeds.child(chip(
                SPEED_CHIP_IDS[index],
                SPEED_LABELS[index],
                (rate - option).abs() < 1e-3,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.apply_speed_op(
                        EditOp::SetSpeed {
                            clip: clip_id,
                            speed: option,
                        },
                        format!("倍率 {}×", format_speed_label(option)),
                        cx,
                    );
                }),
            ));
        }

        let mut holds = div().flex().flex_wrap().gap(px(4.0));
        for (index, option) in HOLD_CHOICES.into_iter().enumerate() {
            holds = holds.child(chip(
                HOLD_CHIP_IDS[index],
                HOLD_LABELS[index],
                hold == option,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    let message = if option == Tick::ZERO {
                        "已取消片尾定格".to_string()
                    } else {
                        format!("片尾定格 {:.2} 秒", option.secs())
                    };
                    this.apply_speed_op(
                        EditOp::SetHold {
                            clip: clip_id,
                            hold: option,
                        },
                        message,
                        cx,
                    );
                }),
            ));
        }

        let summary = format!(
            "片段 {:.2} 秒 = 素材 {:.2} 秒 ÷ {:.2}× + 定格 {:.2} 秒",
            duration.secs(),
            span.secs(),
            rate,
            hold.secs()
        );
        let footer = if locked {
            "轨道已锁定，先解锁再改".to_string()
        } else {
            "变速后同轨后面的片段会跟着让位（波纹）；导出走 setpts / atempo".to_string()
        };

        Some(
            div()
                .id("timeline-speed-menu")
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
                .gap(px(5.0))
                // 与其它面板一样：点面板本身不能漏到下面的时间线去。
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
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(text_faint())
                        .child(summary),
                )
                .child(row_label("倍率"))
                .child(speeds)
                .child(row_label("片尾定格"))
                .child(holds)
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

/// 小标题：和 chip 行一样高，上下留白靠外层 gap。
fn row_label(text: &'static str) -> AnyElement {
    div()
        .text_size(px(10.0))
        .text_color(text_faint())
        .child(text)
        .into_any_element()
}

/// 倍率的展示写法：去掉没用的零，和 chip 上的「1×」「1.5×」对得上。
fn format_speed_label(speed: f32) -> String {
    let text = format!("{speed:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// chip 的 id 必须是静态串：GPUI 靠它做命中测试，每次渲染换一个就等于换了元素。
const SPEED_CHIP_IDS: [&str; 6] = [
    "timeline-speed-025",
    "timeline-speed-050",
    "timeline-speed-100",
    "timeline-speed-150",
    "timeline-speed-200",
    "timeline-speed-400",
];
const SPEED_LABELS: [&str; 6] = ["0.25×", "0.5×", "1×", "1.5×", "2×", "4×"];
const HOLD_CHIP_IDS: [&str; 5] = [
    "timeline-hold-none",
    "timeline-hold-500",
    "timeline-hold-1000",
    "timeline-hold-2000",
    "timeline-hold-3000",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 倍率标签去掉多余的零() {
        let labels: Vec<String> = ClipItem::SPEEDS
            .iter()
            .map(|speed| format_speed_label(*speed))
            .collect();
        assert_eq!(labels, ["0.25", "0.5", "1", "1.5", "2", "4"]);
        // 与 chip 上的静态文案逐字对上：面板里的提示不能和按钮说两样话。
        for (label, chip) in labels.iter().zip(SPEED_LABELS) {
            assert_eq!(format!("{label}×"), chip);
        }
    }

    #[test]
    fn 定格档位从取消开始递增() {
        assert_eq!(HOLD_CHOICES[0], Tick::ZERO, "第一档是「不定格」");
        assert!(
            HOLD_CHOICES.windows(2).all(|pair| pair[0] < pair[1]),
            "档位必须单调递增，否则 chip 的选中态会指错"
        );
        assert_eq!(HOLD_CHOICES.len(), HOLD_LABELS.len());
        assert_eq!(HOLD_CHOICES.len(), HOLD_CHIP_IDS.len());
    }
}
