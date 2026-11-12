//! 时间线轨道：片段行的绘制、拖动换轨、轨道开关、删轨与播放头标注。
//!
//! 按职责分文件，免得一个文件既画界面又管编辑：
//!
//! - `lane` / `label`：一条轨道的两半——右边的片段区与左边的名称列。
//! - `track_ops`：加轨、删轨、转场、静音与锁定。
//! - `clip_ops`：选中、删除、切割。
//! - `markers`：播放头、入出点与标记。
//!
//! 共享的只有拖动类型与几个换算函数，都留在本文件。

mod clip_ops;
mod drop_ops;
mod label;
mod lane;
mod markers;
mod track_ops;

use super::*;
use crate::timeline::{EditOp, Id, Placement, Rational, apply as apply_edit};

/// 拖动中的片段。GPUI 的拖放按值的类型分发，专用类型不会和「从资源管理器拖文件进来」混在一起。
pub(crate) struct TimelineDrag {
    pub(crate) clip: Id,
    /// 抓取点在片段内部的横向像素偏移。落点按片段左边缘算，不按鼠标位置算，
    /// 否则抓住长片段中间拖动时，片段会整个跳到鼠标下面。
    pub(crate) grab: Rc<Cell<f32>>,
    /// 最近一次 `on_drag_move` 里读到的修饰键 `(alt, shift, ctrl)`。
    ///
    /// 松手时拿不到键盘状态，而它们决定「移动 / 滑移 / 滑动 / 插入」几种语义，
    /// 只能在拖动过程中记下来。
    pub(crate) mods: Rc<Cell<(bool, bool, bool)>>,
}

/// 跟着鼠标走的拖动预览：一颗只显示名字的胶囊。
pub(crate) struct TimelineGhost {
    pub(crate) name: SharedString,
}

/// 拖动片段上的关键帧标记。一根标记代表「片段内这个时刻的所有属性」，
/// 所以拖的是整组点，不是某条曲线上的单个点。
pub(crate) struct KeyframeDrag {
    pub(crate) clip: Id,
    /// 抓取时这个点在片段内的相对时间。
    pub(crate) from: Tick,
}

/// 拖动关键帧时算出来的落点：片段内相对时间，已经夹进片段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct KeyframeDrop {
    pub(crate) clip: Id,
    pub(crate) from: Tick,
    pub(crate) to: Tick,
}

/// 片段两端的修剪抓手拖的是哪一端。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrimSide {
    /// 左端：改素材入点。
    Start,
    /// 右端：改素材出点。
    End,
}

/// 拖动片段两端的修剪抓手。事件类型独立，才不会和「拖整个片段」混在一起。
pub(crate) struct TrimDrag {
    pub(crate) clip: Id,
    pub(crate) side: TrimSide,
    /// 抓取点在抓手内部的横向像素偏移。
    pub(crate) grab: Rc<Cell<f32>>,
}

/// 修剪工具：决定「直接拖」走哪种语义，修饰键仍然可以临时覆盖它。
///
/// 键位与常见 NLE 对齐（`V` 选择 / `B` 波纹 / `R` 滚动 / `Y` 滑移 / `U` 滑动）；这里
/// 没有「选择工具」那种全局模式切换，`V` 就是「普通修剪 + 移动」。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TrimTool {
    /// 普通修剪：拖两端改入出点，拖中部移动片段。
    #[default]
    Trim,
    /// 波纹修剪：收短之后同轨后面的片段补位。
    Ripple,
    /// 滚动编辑：只挪接缝，两侧片段一起变长短。
    Roll,
    /// 滑移：位置与时长不动，素材内容左右挪。
    Slip,
    /// 滑动：片段左右挪，两侧邻居跟着伸缩。
    Slide,
}

impl TrimTool {
    pub(crate) const ALL: [TrimTool; 5] = [
        TrimTool::Trim,
        TrimTool::Ripple,
        TrimTool::Roll,
        TrimTool::Slip,
        TrimTool::Slide,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            TrimTool::Trim => "修剪",
            TrimTool::Ripple => "波纹",
            TrimTool::Roll => "滚动",
            TrimTool::Slip => "滑移",
            TrimTool::Slide => "滑动",
        }
    }

    /// 点一下工具指示器时切到下一个（键盘才是直接选）。
    pub(crate) fn next(self) -> Self {
        let index = Self::ALL.iter().position(|tool| *tool == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }
}

/// 落手时实际用哪种修剪：**修饰键优先于工具**，工具又受「这一端能不能这么剪」约束。
///
/// - `Alt` → 波纹修剪（任何一端都成立）。
/// - `Shift` 拖右端且后面紧挨着片段 → 滚动编辑。
/// - 工具是滚动但拖的是左端（`Roll` 只认本段与后一段的接缝）→ 退回普通修剪。
/// - 工具是滑移 / 滑动（都是体上操作）→ 端点拖动退回普通修剪。
pub(crate) fn effective_trim(
    tool: TrimTool,
    side: TrimSide,
    adjacent_next: bool,
    alt: bool,
    shift: bool,
) -> TrimTool {
    if alt {
        return TrimTool::Ripple;
    }
    if shift && side == TrimSide::End && adjacent_next {
        return TrimTool::Roll;
    }
    match (tool, side) {
        (TrimTool::Roll, TrimSide::Start) => TrimTool::Trim,
        (TrimTool::Roll, TrimSide::End) if !adjacent_next => TrimTool::Trim,
        (TrimTool::Slip | TrimTool::Slide, _) => TrimTool::Trim,
        (tool, _) => tool,
    }
}

/// 修剪拖到什么位置、按着哪些修饰键。松手时才真正改序列。
///
/// 修饰键在 `on_drag_move` 里读到就存下来：`on_drop` 只拿得到拖拽值，拿不到当时的
/// 键盘状态，而 Alt / Shift 决定了这是普通修剪、波纹修剪还是滚动编辑。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TrimDrop {
    pub(crate) clip: Id,
    pub(crate) side: TrimSide,
    /// 目标时刻（已对齐到帧）。
    pub(crate) at: Tick,
    pub(crate) alt: bool,
    pub(crate) shift: bool,
}

impl Render for TimelineGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(180.0))
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(4.0))
            .bg(gpui_kit::rgba(0x232330f2))
            .border_1()
            .border_color(ui::accent())
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_size(px(11.0))
            .text_color(text())
            .child(self.name.clone())
    }
}

/// 拖动片段时算出来的落点。渲染预览与松手后执行都用它。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TimelineDrop {
    pub(crate) clip: Id,
    pub(crate) to_track: Id,
    pub(crate) start: Tick,
    /// 落点压到别的片段时怎么处理：默认覆盖，按住 `Shift` 是插入。
    pub(crate) mode: Placement,
}

/// 片段底色：视频蓝、音频绿、字幕橙。整块颜色比图标更容易在缩放很小时认出来。
fn clip_color(kind: crate::timeline::TrackKind) -> gpui_kit::Rgba {
    match kind {
        crate::timeline::TrackKind::Video => gpui_kit::rgba(0x3f6fd8e6),
        crate::timeline::TrackKind::Audio => gpui_kit::rgba(0x2f8f5be6),
        crate::timeline::TrackKind::Subtitle => gpui_kit::rgba(0xb07a2ee6),
    }
}

/// 落点对齐到帧：像素换算出来的时间是连续值，不对齐会得到「半帧」的起点，
/// 导出时那一帧要么重复要么丢掉。
///
/// 素材库拖拽（`app::library::drop`）也用它，两条路径落下来位置一致。
pub(crate) fn snap_to_frame(tick: Tick, fps: Rational) -> Tick {
    Tick::from_frames(tick.round_frames(fps), fps)
}

/// 设完入点 / 出点之后提示什么：区间完整就报区间，否则说明还差另一半。
fn range_message(project: &Project, waiting: &str) -> String {
    match project.ui.range() {
        Some((start, end)) => format!(
            "工作区 {} – {}",
            format_time_precise(start.secs()),
            format_time_precise(end.secs())
        ),
        None => waiting.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 修饰键优先于修剪工具() {
        // Alt 拖任何一端都是波纹，不管当前工具是什么。
        assert_eq!(
            effective_trim(TrimTool::Trim, TrimSide::Start, false, true, false),
            TrimTool::Ripple
        );
        assert_eq!(
            effective_trim(TrimTool::Slip, TrimSide::End, true, true, false),
            TrimTool::Ripple
        );
        // Shift 拖右端且后面紧挨着：滚动编辑，也压过工具。
        assert_eq!(
            effective_trim(TrimTool::Slip, TrimSide::End, true, false, true),
            TrimTool::Roll
        );
        // 什么都不按就按工具走。
        assert_eq!(
            effective_trim(TrimTool::Ripple, TrimSide::End, false, false, false),
            TrimTool::Ripple
        );
        assert_eq!(
            effective_trim(TrimTool::Trim, TrimSide::End, false, false, false),
            TrimTool::Trim
        );
    }

    #[test]
    fn 端点拖不成立的工具退回普通修剪() {
        // 滚动只认本段与后一段的接缝：拖左端没有接缝可挪。
        assert_eq!(
            effective_trim(TrimTool::Roll, TrimSide::Start, true, false, false),
            TrimTool::Trim
        );
        // 拖右端但后面没有紧挨着的片段。
        assert_eq!(
            effective_trim(TrimTool::Roll, TrimSide::End, false, false, false),
            TrimTool::Trim
        );
        // 滑移 / 滑动都是体上操作，端点拖动退回普通修剪。
        assert_eq!(
            effective_trim(TrimTool::Slip, TrimSide::End, true, false, false),
            TrimTool::Trim
        );
        assert_eq!(
            effective_trim(TrimTool::Slide, TrimSide::Start, true, false, false),
            TrimTool::Trim
        );
        // Shift 拖左端不是滚动（只有右端有接缝）。
        assert_eq!(
            effective_trim(TrimTool::Trim, TrimSide::Start, true, false, true),
            TrimTool::Trim
        );
    }

    #[test]
    fn 工具按顺序轮换且能回到修剪() {
        let mut tool = TrimTool::Trim;
        let mut seen = vec![tool];
        for _ in 0..TrimTool::ALL.len() - 1 {
            tool = tool.next();
            seen.push(tool);
        }
        assert_eq!(
            seen,
            vec![
                TrimTool::Trim,
                TrimTool::Ripple,
                TrimTool::Roll,
                TrimTool::Slip,
                TrimTool::Slide
            ]
        );
        assert_eq!(tool.next(), TrimTool::Trim, "转一圈回到默认工具");
    }

    #[test]
    fn 落点吸附到最近的帧() {
        let fps = Rational::FPS_25;
        assert_eq!(
            snap_to_frame(Tick::from_millis(10), fps),
            Tick::ZERO,
            "0.01 秒贴着第 0 帧"
        );
        assert_eq!(
            snap_to_frame(Tick::from_millis(30), fps),
            Tick::from_millis(40),
            "0.03 秒更靠近第 1 帧"
        );
        assert_eq!(
            snap_to_frame(Tick::from_millis(49), fps),
            Tick::from_millis(40)
        );
    }
}
