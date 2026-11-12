//! 播放头定位、滚动与缩放（鼠标滚轮 / 键盘 / 适应整条序列）。

use super::*;

impl PlayerApp {
    /// 把窗口横坐标换成时间线时间，并移动播放头。
    ///
    /// 不调用播放器的 seek：工程模式下时间线是预览，拖动播放头只改工程里的位置。
    /// `snap` 是按住 Shift：落点对齐到 0.01 秒网格，读数稳定可复现。
    pub(in crate::app) fn seek_timeline_to(
        &mut self,
        x: Pixels,
        snap: bool,
        cx: &mut Context<Self>,
    ) {
        // 拖播放头等于用户自己接管定位：正在放就停下，否则时钟会和手指抢播放头，
        // 松手之后播放头还在往前跑。
        self.stop_timeline_playback(cx);
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let lane_x = f32::from(x) - TIMELINE_GUTTER;
        let duration = project.sequence().duration();
        let seconds = seek_seconds(&self.timeline, lane_x, duration, snap);
        self.timeline_snap = snap;
        let mut at = Tick::from_secs(seconds);
        // 磁吸与 Shift 网格互斥：两套落点规则叠在一起会互相抢，按住 Shift 时
        // 用户要的就是精确读数，不该再被拉到标记上。
        if !snap {
            let targets: Vec<Tick> = project
                .ui
                .in_point
                .into_iter()
                .chain(project.ui.out_point)
                .chain(project.sequence().markers.iter().map(|marker| marker.at))
                .collect();
            at = snap_to_targets(&self.timeline, at, &targets);
        }
        if let Some(project) = self.project.as_mut() {
            project.ui.playhead = at;
        }
        // 播放头动了，画面跟着走（M4-T01）。
        self.preview_at_playhead(cx);
        // 拖着走的时候顺带出声（scrub）：指针停住 150 ms 后自己按住。
        if self.timeline_dragging {
            self.scrub_audio(cx);
        }
    }

    pub(in crate::app) fn scroll_timeline(&mut self, pixels: f32) {
        let Some(duration) = self.project.as_ref().map(|p| p.sequence().duration()) else {
            return;
        };
        self.timeline.scroll_by(pixels, duration);
    }

    /// 以 `anchor_x`（轨道区内的横坐标）为锚点缩放。
    pub(in crate::app) fn zoom_timeline(&mut self, factor: f32, anchor_x: f32) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let max_zoom = ViewState::max_zoom(project.sequence().fps);
        let duration = project.sequence().duration();
        self.timeline
            .zoom_around(factor, anchor_x.max(0.0), max_zoom, duration);
    }

    /// 键盘缩放：锚点是播放头。比鼠标缩放粗一档，`-` / `=` 各按一次就是一级。
    pub(in crate::app) fn zoom_timeline_step(&mut self, zoom_in: bool, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let anchor = self.timeline.x_of(project.ui.playhead.secs());
        self.zoom_timeline(if zoom_in { 1.25 } else { 0.8 }, anchor);
        cx.notify();
    }

    /// 缩放到整条序列刚好可见，右侧留 5% 余量。
    pub(in crate::app) fn fit_timeline(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let duration = project.sequence().duration();
        let seconds = duration.secs().max(1.0) as f32;
        let zoom = self.timeline.width() / seconds * 0.95;
        let max_zoom = ViewState::max_zoom(project.sequence().fps);
        self.timeline.set_zoom(zoom, max_zoom, duration);
        self.timeline.scroll_to(0.0, duration);
        cx.notify();
    }
}
