//! 播放头相关的标注：工作区入出点与标记。

use super::*;

/// 时间线上的命中容差：播放头落在目标左右这么多像素内就算「回到原处」。
///
/// 标记、入点、出点都靠它判断「再按一次是删掉还是新建」。一像素太苛刻——
/// 手一抖就差半帧，还是删不掉；再大一点又会误伤刚加在旁边的另一个。
/// 四个像素是能瞄得住、又不会吃掉隔壁目标的距离。
const TIMELINE_HIT_PX: f64 = 4.0;

impl PlayerApp {
    /// 鼠标命中容差：几个屏幕像素换算成时间，跟着缩放走。
    ///
    /// 标记、入点、出点都拿它判断「播放头是不是回到了同一个位置」。
    fn hit_tolerance(&self) -> Tick {
        Tick::from_secs(self.timeline.pixel_seconds() * TIMELINE_HIT_PX)
    }

    /// 把播放头设成工作区入点；播放头已经停在入点上就清掉它。
    pub(crate) fn set_in_point(&mut self, cx: &mut Context<Self>) {
        let tolerance = self.hit_tolerance();
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let at = project.ui.playhead;
        let had = project.ui.in_point;
        project.ui.set_in(at, tolerance);
        let message = if had.is_some() && project.ui.in_point.is_none() {
            "已清除入点".to_string()
        } else {
            range_message(project, "已设入点，等待出点")
        };
        self.flash(message);
        cx.notify();
    }

    /// 把播放头设成工作区出点；播放头已经停在出点上就清掉它。
    pub(crate) fn set_out_point(&mut self, cx: &mut Context<Self>) {
        let tolerance = self.hit_tolerance();
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let at = project.ui.playhead;
        let had = project.ui.out_point;
        project.ui.set_out(at, tolerance);
        let message = if had.is_some() && project.ui.out_point.is_none() {
            "已清除出点".to_string()
        } else {
            range_message(project, "已设出点，等待入点")
        };
        self.flash(message);
        cx.notify();
    }

    /// 清除工作区（入点与出点一起）。
    ///
    /// 标尺右键菜单里的兜底入口：不看播放头停在哪儿，缩放再粗也能清干净。
    pub(crate) fn clear_work_area(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        if project.ui.in_point.is_none() && project.ui.out_point.is_none() {
            self.flash("还没有设置工作区");
            cx.notify();
            return;
        }
        project.ui.clear_range();
        self.flash("已清除工作区");
        cx.notify();
    }

    /// 在播放头处加一个标记；附近已经有标记就删掉它。
    ///
    /// 容差按当前缩放算：拖回来的播放头只能停在像素上，精确时间几乎不可能重合，
    /// 严格相等的话标记就只有加、删不掉。
    pub(crate) fn toggle_marker_at_playhead(&mut self, cx: &mut Context<Self>) {
        let tolerance = self.hit_tolerance();
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let at = project.ui.playhead;
        let name = format!("标记 {}", project.sequence.markers.len() + 1);
        let added = project.sequence.toggle_marker(at, name, tolerance);

        self.flash(if added {
            "已添加标记"
        } else {
            "已删除标记"
        });
        self.poke();
        cx.notify();
    }
}
