//! 时间线片段的右键菜单：贴在鼠标位置，作用对象是刚点中的那一段片段。
//!
//! 只做「手上这一段」相关的事：在播放头处切开、删除、转场、把所在轨道关掉 /
//! 锁住，以及删掉整条轨道。
//! 需要跨轨道批量处理的操作（排序、整体重排）留给菜单栏，这里不堆。
//!
//! 外壳复用播放列表行右键菜单的样式（`widgets::item_menu_entry`）：同一个界面上
//! 的右键菜单长得一样，用户不用重新认一遍。
//!
//! 按职责分文件：本文件只放菜单的枚举、上下文结构与打开逻辑；条目菜单在
//! `clip` / `ruler`，chip 面板（转场 / 轨道）在 `panels`。

mod clip;
mod panels;
mod ruler;

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
    /// 轨道头右键：音量、独奏、静音 / 隐藏与锁定。轨道头只有 108 像素宽，
    /// 放不下第四个开关，这些状态收进一张小面板。
    Track(TrackMenu),
}

/// 轨道菜单的上下文：哪条轨道、鼠标位置。
#[derive(Clone, Copy, Debug)]
pub(super) struct TrackMenu {
    track: Id,
    position: Point<Pixels>,
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
            TimelineMenu::Track(menu) => self.render_track_menu(menu, window, cx),
        }
    }

    /// 轨道头右键菜单。轨道被删掉时跟着收掉，不留一张指向不存在轨道的壳。
    pub(super) fn open_track_menu(
        &mut self,
        track: Id,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() {
            return;
        }
        self.timeline_menu = Some(TimelineMenu::Track(TrackMenu { track, position }));
        cx.notify();
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
        if crate::timeline::transition_room(project.sequence(), after).is_none() {
            self.flash("左边没有紧挨着的片段，加不了转场（先把两段接上）");
            cx.notify();
            return;
        }
        self.timeline_menu = Some(TimelineMenu::Transition(TransitionMenu { after, position }));
        cx.notify();
    }
}

/// 菜单至少离窗口边缘留这么多：贴着边框圆角看不清，也容易点到窗口外面去。
const MENU_MARGIN: f32 = 8.0;

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
