//! 片段关键帧面板：缩放、位置、不透明度按播放头记录取值。
//!
//! 写法取「在播放头处记一个点」：把播放头停到要看的位置，点 ± 调值或点「记」留一个点，
//! 挪到下一个位置再来一次。面板不画曲线，也不做贝塞尔手柄——那属于曲线编辑器。
//!
//! 关键帧的时间存的是**片段内相对时间**，所以拖片段、切片段都不用重排动画。

use super::*;
use crate::timeline::{Id, Interpolation, Property, Tick};

/// 面板上下文：作用在哪个片段、贴在哪里。
#[derive(Clone, Copy, Debug)]
pub(super) struct KeysMenu {
    clip: Id,
    position: Point<Pixels>,
    /// 曲线编辑器正在画哪条属性（点面板上的小标签切换）。
    pub(super) property: Property,
}

const PANEL_WIDTH: f32 = 320.0;

impl PlayerApp {
    pub(super) fn open_keys_menu(
        &mut self,
        clip: Id,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        if project.sequence().clip(clip).is_none() {
            return;
        }
        self.timeline_menu = Some(TimelineMenu::Keys(KeysMenu {
            clip,
            position,
            property: Property::Scale,
        }));
        cx.notify();
    }

    /// 关键帧的改动统一收口：拷一份序列改，成功才落一步撤销。
    pub(super) fn edit_keys(
        &mut self,
        clip: Id,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut crate::timeline::Sequence) -> Option<String>,
    ) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let locked = project
            .sequence()
            .clip_track(clip)
            .and_then(|track| project.sequence().track(track))
            .is_some_and(|track| track.locked);
        if locked {
            self.flash("轨道锁着，先解锁再改关键帧");
            cx.notify();
            return;
        }
        let mut sequence = project.sequence().clone();
        let Some(message) = change(&mut sequence) else {
            return;
        };
        self.commit_edit(sequence, message.clone());
        self.flash(message);
        self.poke();
        cx.notify();
    }

    pub(super) fn render_keys_menu(
        &self,
        menu: KeysMenu,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let project = self.project.as_ref()?;
        let clip = project.sequence().clip(menu.clip)?;
        let track = project
            .sequence()
            .clip_track(menu.clip)
            .and_then(|track| project.sequence().track(track));
        let locked = track.is_some_and(|track| track.locked);
        let title = format!(
            "关键帧 · {}",
            super::timeline_effects::clip_title(project, clip)
        );

        let duration = clip.duration();
        // 播放头落在片段之外时贴到两端：面板上永远有一个「当前时刻」。
        let playhead = project.ui.playhead;
        let offset = (playhead - clip.start).max(Tick::ZERO).min(duration);
        let keys = clip.keys.clone();
        let clip_id = menu.clip;

        let curve_rows = keys
            .curves
            .iter()
            .filter(|curve| !curve.keys.is_empty())
            .count();
        // 标题 + 时刻 + 4 个属性 + 有点的属性 + 插值 + 提示。
        let rows = 3 + Property::ALL.len() + curve_rows + 1;
        // 曲线编辑器（标签 + 画布 + 预设）比一行高得多，按估算的高度让菜单自己翻边。
        let origin = super::timeline_menu::menu_origin(
            menu.position,
            super::timeline_menu::menu_height(rows, 0) + CURVE_BLOCK_HEIGHT,
            window,
        );

        let mut body = div().flex().flex_col().gap(px(4.0));
        for (index, property) in Property::ALL.into_iter().enumerate() {
            let value = keys.value(property, offset);
            let mut row = div()
                .flex()
                .items_center()
                .gap(px(5.0))
                .h(px(24.0))
                .child(
                    div()
                        .w(px(64.0))
                        .text_size(px(11.0))
                        .text_color(text_muted())
                        .child(property.label()),
                )
                .child(
                    div()
                        .w(px(48.0))
                        .text_size(px(11.0))
                        .text_color(text())
                        .child(property.format(value)),
                );

            for (label, sign, prefix) in [("－", -1.0f32, "key-minus"), ("＋", 1.0f32, "key-plus")]
            {
                row = row.child(
                    div()
                        .id((prefix, index))
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded(px(5.0))
                        .bg(ui::track())
                        .text_size(px(11.0))
                        .text_color(text_muted())
                        .cursor(CursorStyle::PointingHand)
                        .child(label)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.edit_keys(clip_id, cx, |sequence| {
                                let item = sequence.clip_mut(clip_id)?;
                                let at = offset.min(item.duration());
                                let current = item.keys.value(property, at);
                                let next = (current + sign * property.step())
                                    .clamp(property.min(), property.max());
                                item.keys.ensure(property).insert(at, next);
                                Some(format!("{}：{}", property.label(), property.format(next)))
                            });
                        })),
                );
            }
            row = row.child(
                div()
                    .id(("key-mark", index))
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(5.0))
                    .bg(ui::accent())
                    .text_size(px(11.0))
                    .text_color(bg())
                    .cursor(CursorStyle::PointingHand)
                    .child("记")
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.edit_keys(clip_id, cx, |sequence| {
                            let item = sequence.clip_mut(clip_id)?;
                            let at = offset.min(item.duration());
                            let current = item.keys.value(property, at);
                            let added = item.keys.ensure(property).insert(at, current);
                            Some(if added {
                                format!(
                                    "{}：在 {:.2} 秒记下 {}",
                                    property.label(),
                                    at.secs(),
                                    property.format(current)
                                )
                            } else {
                                format!("{}：这个时刻已经有关键帧", property.label())
                            })
                        });
                    })),
            );
            body = body.child(row);

            // 已经打过的点：点一下删掉。时间写秒，方便和播放头对照。
            if let Some(curve) = keys.curve(property)
                && !curve.keys.is_empty()
            {
                let mut points = div().flex().flex_wrap().gap(px(3.0)).child(
                    div()
                        .text_size(px(10.0))
                        .text_color(text_faint())
                        .child("点"),
                );
                for (point_index, key) in curve.keys.iter().enumerate() {
                    let at = key.at;
                    points = points.child(
                        div()
                            .id(("key-point", index * 16 + point_index))
                            .px(px(5.0))
                            .py(px(1.0))
                            .rounded(px(4.0))
                            .bg(ui::track())
                            .text_size(px(10.0))
                            .text_color(text_muted())
                            .cursor(CursorStyle::PointingHand)
                            .child(format!("{:.2}s {}", at.secs(), property.format(key.value)))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                this.edit_keys(clip_id, cx, |sequence| {
                                    let item = sequence.clip_mut(clip_id)?;
                                    item.keys.ensure(property).remove_near(at, Tick::ZERO).then(
                                        || {
                                            format!(
                                                "{}：删掉 {:.2} 秒的点",
                                                property.label(),
                                                at.secs()
                                            )
                                        },
                                    )
                                });
                            })),
                    );
                }
                body = body.child(points);
            }
        }

        // 插值对整段生效：一条曲线一套插值反而让人不知道自己在改哪条。
        let current = keys
            .curves
            .iter()
            .find(|curve| curve.animated())
            .map(|curve| curve.interpolation)
            .unwrap_or_default();
        let mut interpolation = div().flex().items_center().gap(px(4.0)).child(
            div()
                .w(px(40.0))
                .text_size(px(11.0))
                .text_color(text_faint())
                .child("插值"),
        );
        for (index, option) in Interpolation::ALL.into_iter().enumerate() {
            let active = option == current;
            interpolation = interpolation.child(chip(
                INTERPOLATION_CHIP_IDS[index],
                option.label(),
                active,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.edit_keys(clip_id, cx, |sequence| {
                        let item = sequence.clip_mut(clip_id)?;
                        for curve in &mut item.keys.curves {
                            curve.interpolation = option;
                            // 曲线上逐点的自定义缓动也一起抹掉：选了「线性」却还有两段
                            // 在按贝塞尔走，用户会觉得按钮没生效。
                            for key in &mut curve.keys {
                                key.easing = None;
                            }
                        }
                        Some(format!("插值：{}", option.label()))
                    });
                }),
            ));
        }
        body = body.child(interpolation);

        // 曲线编辑器：画当前属性的曲线、标时刻与取值、拖贝塞尔控制点。
        body = body.child(self.render_curve_editor(clip_id, menu.property, offset, duration, cx));

        let footer = if locked {
            "轨道已锁定，先解锁再改".to_string()
        } else if keys.is_empty() {
            "还没有关键帧：把播放头停到位置，点 ＋ 或「记」".to_string()
        } else {
            "至少两个点才插值；导出时推近走 zoompan，淡入淡出走 fade".to_string()
        };

        Some(
            div()
                .id("timeline-keys-menu")
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
                        .child(format!(
                            "片段内 {:.2} / {:.2} 秒（播放头位置）",
                            offset.secs(),
                            duration.secs()
                        )),
                )
                .child(body)
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

const INTERPOLATION_CHIP_IDS: [&str; 2] = ["key-interp-linear", "key-interp-ease"];

/// 曲线编辑器那块的大致高度：标签行 + 画布 + 预设行 + 间距。
pub(super) const CURVE_BLOCK_HEIGHT: f32 = 244.0;
