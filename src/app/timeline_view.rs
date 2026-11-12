//! 工程模式的时间线：标题条、标尺、播放头与工程会话（新建 / 打开 / 关闭）。
//!
//! 轨道行、片段绘制、拖动换轨与轨道开关在 `timeline_tracks.rs`。缩放 / 滚动 /
//! 坐标换算的数字在 `crate::timeline::ViewState` 里，能用单测钉住；写盘与未保存
//! 确认在 `project_file.rs`。
//!
//! 布局固定为「上方预览 + 下方时间线」：进入工程模式后右侧面板与控制条都不画，
//! 画面区让出底部这一条（见 `render_video_area` 与 `controls_visible`）。

use super::*;
use crate::timeline::Id;

mod tracks_area;

impl PlayerApp {
    /// 时间线占多高。窗口高时按比例，窄窗口不低于 170，宽屏不超过 320。
    pub(super) fn timeline_height(&self, window: &Window) -> f32 {
        if self.project.is_none() || window.is_fullscreen() {
            return 0.0;
        }
        let available = f32::from(window.viewport_size().height) - MENUBAR_HEIGHT;
        // 窗口很矮时优先保证预览还剩一块地方，时间线可以缩到 120。
        let ceiling = (available - 120.0).max(120.0);
        (available * 0.34)
            .clamp(TIMELINE_MIN_HEIGHT, TIMELINE_MAX_HEIGHT)
            .min(ceiling)
    }

    /// 按窗口宽度同步视口宽度，并按面板高度夹住轨道的垂直滚动。
    pub(super) fn sync_timeline_view(&mut self, window: &Window) {
        let height = self.timeline_height(window);
        if height > 0.0 {
            let total = self
                .project
                .as_ref()
                .map_or(0, |project| project.sequence().tracks.len());
            // 加轨时只记了「要看最后一屏」的意图，面板高度到这里才知道；
            // 删轨 / 窗口变矮之后也在这里把滚动位置夹回合法范围。
            self.timeline.sync_tracks(total, visible_track_rows(height));
        }
        let Some(duration) = self.project.as_ref().map(|p| p.sequence().duration()) else {
            return;
        };
        let width = (f32::from(window.viewport_size().width) - TIMELINE_GUTTER).max(1.0);
        self.timeline.set_width(width, duration);
    }
}

mod document;
mod history_ops;
mod render;
mod zoom;

/// 面板高度能完整放下几条轨道行。至少一条：面板再矮也得画一行。
///
/// 标题条与标尺占掉的高度不算进轨道区，否则最后一条会被裁成半截。
fn visible_track_rows(height: f32) -> usize {
    let usable = (height - TIMELINE_HEADER - TIMELINE_RULER).max(0.0);
    (usable / (TIMELINE_TRACK_HEIGHT + TIMELINE_TRACK_GAP))
        .floor()
        .max(1.0) as usize
}

/// 撤销 / 重做之后，选中集合里指向已消失片段的 id 要清掉，
/// 否则下一次删除会拿一个不存在的 id 去查表并报错。
fn prune_selection(selected: &mut Vec<Id>, alive: &std::collections::HashSet<Id>) {
    selected.retain(|id| alive.contains(id));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{AssetId, ClipItem, Rational, Sequence};

    #[test]
    fn 面板高度决定可见轨道行数() {
        assert_eq!(visible_track_rows(170.0), 2, "(170-62)/48 = 2.25");
        assert_eq!(visible_track_rows(213.0), 3, "213 高的窗口画三条");
        assert_eq!(visible_track_rows(320.0), 5, "(320-62)/48 = 5.375");
        assert_eq!(visible_track_rows(0.0), 1, "全屏 / 空工程也留一条兜底");
    }

    #[test]
    fn 历史回退后清掉指向已删片段的选中() {
        let mut sequence = Sequence::new("测试", Rational::FPS_25, 1920, 1080);
        let track = sequence.tracks[0].id;
        let id = sequence.alloc_id();
        sequence.track_mut(track).unwrap().clips.push(ClipItem::new(
            id,
            AssetId::new(1),
            Tick::ZERO,
            Tick::from_secs(1.0),
            Tick::ZERO,
        ));

        let mut selected = vec![id, Id::new(999)];
        let alive: std::collections::HashSet<Id> =
            sequence.clips().map(|(_, clip)| clip.id).collect();
        prune_selection(&mut selected, &alive);
        assert_eq!(selected, vec![id], "只留下还在序列里的");
    }
}
