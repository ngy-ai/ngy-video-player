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
mod label;
mod lane;
mod markers;
mod track_ops;

use super::*;
use crate::timeline::{EditOp, Id, Rational, apply as apply_edit};

/// 拖动中的片段。GPUI 的拖放按值的类型分发，专用类型不会和「从资源管理器拖文件进来」混在一起。
pub(crate) struct TimelineDrag {
    pub(crate) clip: Id,
    /// 抓取点在片段内部的横向像素偏移。落点按片段左边缘算，不按鼠标位置算，
    /// 否则抓住长片段中间拖动时，片段会整个跳到鼠标下面。
    pub(crate) grab: Rc<Cell<f32>>,
}

/// 跟着鼠标走的拖动预览：一颗只显示名字的胶囊。
pub(crate) struct TimelineGhost {
    pub(crate) name: SharedString,
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
