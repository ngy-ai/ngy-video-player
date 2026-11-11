//! 时间线片段的右键菜单：贴在鼠标位置，作用对象是刚点中的那一段片段。
//!
//! 只做「手上这一段」相关的事：在播放头处切开、删除、转场、把所在轨道关掉 /
//! 锁住，以及删掉整条轨道。
//! 需要跨轨道批量处理的操作（排序、整体重排）留给菜单栏，这里不堆。
//!
//! 外壳复用播放列表行右键菜单的样式（`widgets::item_menu_entry`）：同一个界面上
//! 的右键菜单长得一样，用户不用重新认一遍。

use super::*;
use crate::timeline::Id;

/// 展开中的时间线菜单。
#[derive(Clone, Copy, Debug)]
pub(super) enum TimelineMenu {
    /// 片段右键：作用对象是点中的那一段与它所在的轨道。
    Clip(ClipMenu),
    /// 标尺右键：整条时间线级别的动作，不指向任何片段。
    Ruler { position: Point<Pixels> },
    /// 接缝上的转场面板：类型与时长都在这里点，改一次落一步撤销。
    Transition(TransitionMenu),
    /// 片段的效果栈面板：加、开关、换档、调序、删。
    Effects(super::timeline_effects::EffectsMenu),
    /// 片段的关键帧面板：在播放头处记录缩放 / 位置 / 不透明度。
    Keys(super::timeline_keys::KeysMenu),
    /// 片段的变速与定格面板：倍率档位与片尾定格时长。
    Speed(super::timeline_speed::SpeedMenu),
    /// 片段的调色面板：亮度 / 对比度 / 饱和度 / 色温与 LUT。
    Color(super::timeline_color::ColorMenu),
    /// 字幕轨上的条目右键菜单：编辑或删除这一条文字。
    Subtitle { item: Id, position: Point<Pixels> },
}

/// 转场面板的上下文：接缝右侧的那段片段（转场挂在它身上）与鼠标位置。
#[derive(Clone, Copy, Debug)]
pub(super) struct TransitionMenu {
    after: Id,
    position: Point<Pixels>,
}

/// 片段菜单的上下文：右键点中的那一段、它所在的轨道与鼠标位置。
#[derive(Clone, Copy, Debug)]
pub(super) struct ClipMenu {
    clip: Id,
    track: Id,
    position: Point<Pixels>,
}

impl PlayerApp {
    /// 在片段上右键：先选中它（多选里已经含有时不动），再在鼠标位置弹菜单。
    pub(super) fn open_timeline_menu(
        &mut self,
        clip: Id,
        track: Id,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if let Some(project) = self.project.as_mut() {
            project.ui.selected = vec![clip];
        }
        self.timeline_menu = Some(TimelineMenu::Clip(ClipMenu {
            clip,
            track,
            position,
        }));
        cx.notify();
    }

    /// 标尺上右键：弹出与整条时间线相关的菜单。
    ///
    /// 「清除工作区」放这里当兜底：快捷键那条路要求播放头落回入点 / 出点附近，
    /// 缩得很粗或者手不稳时可能清不掉；这个入口不看播放头。
    pub(super) fn open_ruler_menu(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.project.is_none() {
            return;
        }
        self.timeline_menu = Some(TimelineMenu::Ruler { position });
        cx.notify();
    }

    pub(super) fn render_timeline_menu(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        match self.timeline_menu? {
            TimelineMenu::Clip(menu) => self.render_clip_menu(menu, window, cx),
            TimelineMenu::Ruler { position } => self.render_ruler_menu(position, window, cx),
            TimelineMenu::Transition(menu) => self.render_transition_menu(menu, window, cx),
            TimelineMenu::Effects(menu) => self.render_effects_menu(menu, window, cx),
            TimelineMenu::Keys(menu) => self.render_keys_menu(menu, window, cx),
            TimelineMenu::Speed(menu) => self.render_speed_menu(menu, window, cx),
            TimelineMenu::Color(menu) => self.render_color_menu(menu, window, cx),
            TimelineMenu::Subtitle { item, position } => {
                self.render_subtitle_item_menu(item, position, window, cx)
            }
        }
    }

    /// 打开接缝上的转场面板。左边不是紧邻的片段时直接给提示，不弹一张空面板。
    pub(super) fn open_transition_menu(
        &mut self,
        after: Id,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        if crate::timeline::transition_room(&project.sequence, after).is_none() {
            self.flash("左边没有紧挨着的片段，加不了转场（先把两段接上）");
            cx.notify();
            return;
        }
        self.timeline_menu = Some(TimelineMenu::Transition(TransitionMenu { after, position }));
        cx.notify();
    }

    /// 转场面板：两行互斥 chip（类型、时长）加一颗「去掉转场」。
    ///
    /// 面板保持打开：调类型和时长是一件事的两半，点一下换一个就得重新点开，
    /// 等于逼用户来回三次。每次改动各自落一步撤销。
    fn render_transition_menu(
        &self,
        menu: TransitionMenu,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let project = self.project.as_ref()?;
        // 片段被删掉时菜单跟着收掉，不留一张指向不存在接缝的壳。
        project.sequence.clip(menu.after)?;
        let current = crate::timeline::transition_at(&project.sequence, menu.after);
        let kind = current
            .as_ref()
            .map(|transition| transition.kind)
            .unwrap_or_default();
        let duration = current
            .as_ref()
            .map(|transition| transition.duration)
            .unwrap_or(crate::timeline::DEFAULT_TRANSITION);
        let room = crate::timeline::transition_room(&project.sequence, menu.after)?;
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
                            crate::timeline::transition_at(&project.sequence, after)
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
                            crate::timeline::transition_at(&project.sequence, after)
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

    fn render_clip_menu(
        &self,
        menu: ClipMenu,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let project = self.project.as_ref()?;
        let track = project.sequence.track(menu.track)?;
        // 片段已经被删掉（比如刚按过 Delete）时，菜单跟着收掉，不留一张空壳。
        project.sequence.clip(menu.clip)?;

        let (mute_label, lock_label) = toggle_labels(track.kind, track.muted, track.locked);
        // 图标跟着状态走：亮着的是「当前状态」，点下去变成另一个。
        let mute_icon = match (track.kind, track.muted) {
            (crate::timeline::TrackKind::Audio, false) => IconName::VolumeX,
            (crate::timeline::TrackKind::Audio, true) => IconName::Volume2,
            (_, false) => IconName::EyeOff,
            (_, true) => IconName::Eye,
        };
        let lock_icon = if track.locked {
            IconName::LockOpen
        } else {
            IconName::Lock
        };
        let track_id = menu.track;

        // 转场只对「紧挨着的同轨前一段」成立，文案得照实说，不能让用户点了才发现。
        let index = track.clip_index(menu.clip).unwrap_or(0);
        let adjacent = index > 0 && track.clips[index - 1].end() == track.clips[index].start;
        let has_transition = adjacent
            && track.transitions.iter().any(|transition| {
                transition.before == track.clips[index - 1].id && transition.after == menu.clip
            });
        let transition_label = match (adjacent, has_transition) {
            (false, _) => "左边没有紧挨着的片段",
            (true, true) => "转场设置…",
            (true, false) => "与左侧片段加转场…",
        };
        let clip_id = menu.clip;
        // 片段菜单：10 行 + 1 条分隔线。
        let origin = menu_origin(menu.position, menu_height(10, 1), window);

        Some(
            div()
                .id("timeline-menu")
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
                // 菜单上的点击统一吃掉：别漏到时间线（点空白会取消选中），也别让
                // 根节点的「点别处收菜单」在 mouse-down 阶段就把它拆掉。
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
                    "timeline-menu-split",
                    IconName::Scissors,
                    "在播放头处切开",
                    text(),
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.timeline_menu = None;
                        this.split_at_playhead(cx);
                    }),
                ))
                .child(item_menu_entry(
                    "timeline-menu-delete",
                    IconName::Trash,
                    "删除片段",
                    ui::danger(),
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.timeline_menu = None;
                        this.delete_selected_clips(false, cx);
                    }),
                ))
                // 波纹删除：删掉之后同轨后面的片段往前补，不留空隙。
                .child(item_menu_entry(
                    "timeline-menu-ripple-delete",
                    IconName::Trash,
                    "波纹删除（后面补齐）",
                    ui::danger(),
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.timeline_menu = None;
                        this.delete_selected_clips(true, cx);
                    }),
                ))
                .child(item_menu_entry(
                    "timeline-menu-transition",
                    IconName::Sparkles,
                    transition_label,
                    if adjacent { text() } else { text_faint() },
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.open_transition_menu(clip_id, menu.position, cx);
                    }),
                ))
                .child(item_menu_entry(
                    "timeline-menu-effects",
                    IconName::SlidersHorizontal,
                    "效果…",
                    text(),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.open_effects_menu(clip_id, menu.position, cx);
                    }),
                ))
                .child(item_menu_entry(
                    "timeline-menu-keys",
                    IconName::Waypoints,
                    "关键帧…",
                    text(),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.open_keys_menu(clip_id, menu.position, cx);
                    }),
                ))
                .child(item_menu_entry(
                    "timeline-menu-speed",
                    IconName::Gauge,
                    "变速 / 定格…",
                    text(),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.open_speed_menu(clip_id, menu.position, cx);
                    }),
                ))
                .child(item_menu_entry(
                    "timeline-menu-color",
                    IconName::Palette,
                    "调色…",
                    text(),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.open_color_menu(clip_id, menu.position, cx);
                    }),
                ))
                .child(
                    div()
                        .mx(px(8.0))
                        .my(px(4.0))
                        .h(px(1.0))
                        .bg(gpui_kit::rgb(0x2c2c34)),
                )
                .child(item_menu_entry(
                    "timeline-menu-mute",
                    mute_icon,
                    mute_label,
                    text(),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.timeline_menu = None;
                        this.toggle_track_muted(track_id, cx);
                    }),
                ))
                .child(item_menu_entry(
                    "timeline-menu-lock",
                    lock_icon,
                    lock_label,
                    text(),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.timeline_menu = None;
                        this.toggle_track_locked(track_id, cx);
                    }),
                ))
                .child(item_menu_entry(
                    "timeline-menu-delete-track",
                    IconName::Trash,
                    "删除整条轨道（含片段）",
                    ui::danger(),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.timeline_menu = None;
                        this.delete_track(track_id, cx);
                    }),
                ))
                .into_any_element(),
        )
    }

    /// 标尺菜单。外壳与片段菜单一致，只是内容只有时间线级别的那几项。
    fn render_ruler_menu(
        &self,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let project = self.project.as_ref()?;
        // 没设过工作区时这一项压暗，点下去给一句说明而不是无声无息。
        let has_range = project.ui.in_point.is_some() || project.ui.out_point.is_some();
        let origin = menu_origin(position, menu_height(1, 0), window);

        Some(
            div()
                .id("timeline-ruler-menu")
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
                // 与片段菜单一样：点菜单本身不能漏到下面的标尺去挪播放头。
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
                    "timeline-ruler-clear-range",
                    IconName::Trash,
                    "清除工作区",
                    if has_range { text() } else { text_faint() },
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        this.timeline_menu = None;
                        this.clear_work_area(cx);
                    }),
                ))
                .into_any_element(),
        )
    }
}

/// 菜单至少离窗口边缘留这么多：贴着边框圆角看不清，也容易点到窗口外面去。
const MENU_MARGIN: f32 = 8.0;

/// 转场面板的宽度与估算高度（标题 + 两行 chip + 一行底部）。
const TRANSITION_PANEL_WIDTH: f32 = 252.0;
const TRANSITION_PANEL_HEIGHT: f32 = 120.0;

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

/// 按行数估算菜单高度，只用来判断在鼠标下方放不放得下。
pub(super) fn menu_height(rows: usize, separators: usize) -> f32 {
    ITEM_MENU_PADDING * 2.0
        + rows as f32 * ITEM_MENU_ROW_HEIGHT
        + separators as f32 * ITEM_MENU_SEPARATOR_HEIGHT
}

/// 菜单的实际落点：默认贴在鼠标右下方；下方放不下就翻到鼠标上方，底部仍对齐鼠标，
/// 横向再往窗口里收。
///
/// 时间线贴窗口底边，右键最下面几条轨道时鼠标离下沿往往不到一个菜单的高度，
/// 不翻的话后几项（静音、锁定、删轨道）会整条掉到窗口外，点不到也看不见。
pub(super) fn menu_origin(position: Point<Pixels>, height: f32, window: &Window) -> Point<Pixels> {
    menu_origin_in(position, height, window.viewport_size())
}

/// `menu_origin` 的纯计算部分：不碰窗口，方便单测直接钉住翻面的边界。
fn menu_origin_in(position: Point<Pixels>, height: f32, viewport: Size<Pixels>) -> Point<Pixels> {
    let width = f32::from(viewport.width);
    let viewport_height = f32::from(viewport.height);
    let max_x = (width - ITEM_MENU_WIDTH - MENU_MARGIN).max(MENU_MARGIN);
    let x = f32::from(position.x).clamp(MENU_MARGIN, max_x);

    let y = f32::from(position.y);
    let y = if y + height + MENU_MARGIN > viewport_height {
        (y - height).max(MENU_MARGIN)
    } else {
        y
    };
    point(px(x), px(y))
}

/// 菜单上两行开关的文案。
///
/// 同一个 `muted` 字段，音频轨说「静音」、视频 / 字幕轨说「隐藏」——用户看到的
/// 应该是自己那条轨道上的结果，而不是字段名。抽成纯函数，四种组合好逐个钉住。
fn toggle_labels(
    kind: crate::timeline::TrackKind,
    muted: bool,
    locked: bool,
) -> (&'static str, &'static str) {
    let mute = match (kind, muted) {
        (crate::timeline::TrackKind::Audio, false) => "静音这一轨",
        (crate::timeline::TrackKind::Audio, true) => "恢复这一轨的声音",
        (_, false) => "隐藏这一轨",
        (_, true) => "显示这一轨",
    };
    let lock = if locked {
        "解锁这一轨"
    } else {
        "锁定这一轨"
    };
    (mute, lock)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::TrackKind;

    #[test]
    fn 窗口底部放不下时菜单翻到鼠标上方() {
        let viewport = size(px(1280.0), px(700.0));
        let height = menu_height(7, 1);
        assert!(height > 150.0, "七行菜单估出来太矮：{height}");

        // 鼠标在 600：下面只剩 100 像素，必须翻上去，底部仍贴着鼠标。
        let origin = menu_origin_in(point(px(200.0), px(600.0)), height, viewport);
        assert_eq!(f32::from(origin.y), 600.0 - height);

        // 窗口比菜单还矮：翻上去也放不下，那就贴顶，不许变成负数飘出窗口。
        let short = size(px(1280.0), px(300.0));
        let origin = menu_origin_in(point(px(200.0), px(200.0)), height, short);
        assert_eq!(f32::from(origin.y), MENU_MARGIN);
    }

    #[test]
    fn 空间够就贴在鼠标下方并往窗口里收() {
        let viewport = size(px(1280.0), px(900.0));
        let height = menu_height(1, 0);
        let origin = menu_origin_in(point(px(200.0), px(100.0)), height, viewport);
        assert_eq!(f32::from(origin.x), 200.0);
        assert_eq!(f32::from(origin.y), 100.0);

        // 鼠标贴着右边缘：菜单左移，右边缘离窗口留 MENU_MARGIN。
        let origin = menu_origin_in(point(px(1270.0), px(100.0)), height, viewport);
        assert_eq!(f32::from(origin.x), 1280.0 - ITEM_MENU_WIDTH - MENU_MARGIN);
    }

    #[test]
    fn 音频说静音视频说隐藏() {
        assert_eq!(
            toggle_labels(TrackKind::Audio, false, false),
            ("静音这一轨", "锁定这一轨")
        );
        assert_eq!(
            toggle_labels(TrackKind::Audio, true, false),
            ("恢复这一轨的声音", "锁定这一轨")
        );
        assert_eq!(
            toggle_labels(TrackKind::Video, false, false),
            ("隐藏这一轨", "锁定这一轨")
        );
        assert_eq!(
            toggle_labels(TrackKind::Video, true, true),
            ("显示这一轨", "解锁这一轨")
        );
    }
}
