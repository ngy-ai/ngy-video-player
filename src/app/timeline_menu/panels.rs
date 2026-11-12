//! 两种「多行 chip」面板：转场设置与轨道设置。
//!
//! 它们不是条目列表，而是一排排互斥 / 可切换的 chip，所以单独放一个文件：
//! 片段的条目菜单与标尺菜单都在别的文件里，改这里不会碰到那些。

use super::*;

/// 转场面板的宽度与估算高度（标题 + 两行 chip + 一行底部）。
const TRANSITION_PANEL_WIDTH: f32 = 252.0;
const TRANSITION_PANEL_HEIGHT: f32 = 120.0;

/// 轨道面板的宽度与估算高度（标题 + 音量行 + 开关行）。
const TRACK_PANEL_WIDTH: f32 = 244.0;
const TRACK_PANEL_HEIGHT: f32 = 84.0;

/// 轨道音量的档位：线性增益与标签一一对应（-12 / -6 / 0 / +6 dB）。
///
/// 与导出侧的 `volume` 滤镜同一个数：预览乘到 mpv 音量上，导出写进滤镜链，
/// 两边电平因此能对上（验收标准是差 < 1 dB）。
const TRACK_GAIN_CHOICES: [f32; 4] = [0.2512, 0.5012, 1.0, 1.9953];
const TRACK_GAIN_LABELS: [&str; 4] = ["-12 dB", "-6 dB", "0 dB", "+6 dB"];
const TRACK_GAIN_CHIP_IDS: [&str; 4] = [
    "track-gain-m12",
    "track-gain-m6",
    "track-gain-0",
    "track-gain-p6",
];

/// 时长选项：几档常见的值，真正生效的上限还是两侧余量。
const DURATION_CHOICES: [Tick; 5] = [
    Tick::from_millis(200),
    Tick::from_millis(500),
    Tick::from_millis(1000),
    Tick::from_millis(1500),
    Tick::from_millis(2000),
];
const DURATION_LABELS: [&str; 5] = ["0.2 秒", "0.5 秒", "1 秒", "1.5 秒", "2 秒"];

/// chip 的 id 必须是静态串：GPUI 靠它做命中测试，每次渲染换一个就等于换了元素。
const TYPE_CHIP_IDS: [&str; 4] = [
    "transition-kind-dissolve",
    "transition-kind-fade",
    "transition-kind-wipe",
    "transition-kind-slide",
];
const DURATION_CHIP_IDS: [&str; 5] = [
    "transition-duration-200",
    "transition-duration-500",
    "transition-duration-1000",
    "transition-duration-1500",
    "transition-duration-2000",
];

impl PlayerApp {
    /// 转场面板：类型一排、时长一排、底部是「去掉转场」或余量说明。
    pub(super) fn render_transition_menu(
        &self,
        menu: TransitionMenu,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let project = self.project.as_ref()?;
        // 片段被删掉时菜单跟着收掉，不留一张指向不存在接缝的壳。
        project.sequence().clip(menu.after)?;
        let current = crate::timeline::transition_at(project.sequence(), menu.after);
        let kind = current
            .as_ref()
            .map(|transition| transition.kind)
            .unwrap_or_default();
        let duration = current
            .as_ref()
            .map(|transition| transition.duration)
            .unwrap_or(crate::timeline::DEFAULT_TRANSITION);
        let room = crate::timeline::transition_room(project.sequence(), menu.after)?;
        let after = menu.after;
        let origin = menu_origin(menu.position, TRANSITION_PANEL_HEIGHT, window);

        let title = match &current {
            Some(_) => format!("转场 · {} {:.2} 秒", kind.label(), duration.secs()),
            None => "加转场".to_string(),
        };

        let mut kinds = div().flex().flex_wrap().gap(px(4.0));
        for (index, option) in crate::timeline::TransitionKind::ALL.into_iter().enumerate() {
            kinds = kinds.child(chip(
                TYPE_CHIP_IDS[index],
                option.label(),
                option == kind,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    // 换类型时保留当前时长：用户多半只是想换个效果看看。
                    let duration = this
                        .project
                        .as_ref()
                        .and_then(|project| {
                            crate::timeline::transition_at(project.sequence(), after)
                        })
                        .map(|transition| transition.duration)
                        .unwrap_or(crate::timeline::DEFAULT_TRANSITION);
                    this.apply_transition(after, option, duration, cx);
                }),
            ));
        }

        let mut durations = div().flex().flex_wrap().gap(px(4.0));
        for (index, option) in DURATION_CHOICES.into_iter().enumerate() {
            durations = durations.child(chip(
                DURATION_CHIP_IDS[index],
                DURATION_LABELS[index],
                current
                    .as_ref()
                    .is_some_and(|transition| transition.duration == option),
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    // 换时长时保留当前类型；还没有转场就先按默认类型加一个。
                    let kind = this
                        .project
                        .as_ref()
                        .and_then(|project| {
                            crate::timeline::transition_at(project.sequence(), after)
                        })
                        .map(|transition| transition.kind)
                        .unwrap_or_default();
                    this.apply_transition(after, kind, option, cx);
                }),
            ));
        }

        let footer = if current.is_some() {
            chip(
                "transition-remove",
                "去掉转场",
                false,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.drop_transition(after, cx);
                }),
            )
        } else {
            // 没有转场时这里放余量说明：用户能看见「最多能加多长」。
            div()
                .text_size(px(11.0))
                .text_color(text_faint())
                .child(format!("最多 {:.2} 秒（两侧余量）", room.secs()))
                .into_any_element()
        };

        Some(
            div()
                .id("timeline-transition-menu")
                .absolute()
                .left(origin.x)
                .top(origin.y)
                .w(px(TRANSITION_PANEL_WIDTH))
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
                // 与其它菜单一样：点面板本身不能漏到下面的时间线去。
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
                .child(kinds)
                .child(durations)
                .child(
                    div()
                        .mt(px(2.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(footer),
                )
                .into_any_element(),
        )
    }

    /// 轨道面板：一行音量档位（只对音频轨），一行开关。
    ///
    /// 每次点击各落一步撤销，面板保持打开——调音量常要来回试几档，点一下关一次
    /// 等于逼用户反复右键。
    pub(super) fn render_track_menu(
        &self,
        menu: TrackMenu,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let project = self.project.as_ref()?;
        let track = project.sequence().track(menu.track)?;
        let audio = track.kind == crate::timeline::TrackKind::Audio;
        let gain = track.normalized_gain();
        let muted = track.muted;
        let solo = track.solo;
        let locked = track.locked;
        let title = if audio {
            format!("轨道「{}」 · {}", track.name, track.gain_label())
        } else {
            format!("轨道「{}」", track.name)
        };
        let id = menu.track;
        let origin = menu_origin(menu.position, TRACK_PANEL_HEIGHT, window);

        let mut gains = div().flex().flex_wrap().gap(px(4.0));
        if audio {
            for (index, choice) in TRACK_GAIN_CHOICES.into_iter().enumerate() {
                gains = gains.child(chip(
                    TRACK_GAIN_CHIP_IDS[index],
                    TRACK_GAIN_LABELS[index],
                    (gain - choice).abs() < 0.02,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.set_track_gain(id, choice, cx);
                    }),
                ));
            }
        } else {
            // 视频 / 字幕轨没有音量：把「量」这一行的位置给一句说明，面板高度不变。
            gains = gains.child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_faint())
                    .child("音量与独奏只对音频轨有意义"),
            );
        }

        let mute_label = if audio { "静音" } else { "隐藏" };
        let mut toggles = div().flex().flex_wrap().gap(px(4.0));
        // 独奏只给音频轨：画面是叠出来的，一条视频轨「独奏」没有定义。
        if audio {
            toggles = toggles.child(chip(
                "track-menu-solo",
                "独奏",
                solo,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.toggle_track_solo(id, cx);
                }),
            ));
        }
        let toggles = toggles
            .child(chip(
                "track-menu-mute",
                mute_label,
                muted,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.toggle_track_muted(id, cx);
                }),
            ))
            .child(chip(
                "track-menu-lock",
                "锁定",
                locked,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.toggle_track_locked(id, cx);
                }),
            ));

        Some(
            div()
                .id("timeline-track-menu")
                .absolute()
                .left(origin.x)
                .top(origin.y)
                .w(px(TRACK_PANEL_WIDTH))
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
                // 与其它菜单一样：点面板本身不能漏到下面的轨道去。
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
                .child(gains)
                .child(toggles)
                .into_any_element(),
        )
    }
}
