//! 时间线视图状态：缩放、滚动与「秒 ↔ 像素」换算。
//!
//! 纯数字，不碰 GPUI：界面把视口宽度和鼠标位置传进来，这里只回答「第几秒在哪个
//! 像素」「滚一格走多远」「标尺该用多大刻度」。三小时工程的滚动边界和缩放锚点
//! 因此可以用单测钉住，不必开窗口反复试。
//!
//! 缩放范围：最粗 `1 像素 = 10 秒`（[`MIN_ZOOM`]），最细一帧占 8 像素
//! （[`ViewState::max_zoom`]）。到帧级时标尺改铺帧刻度，见 [`visible_frames`]。

use crate::timeline::{Rational, Tick};
use std::ops::Range;

/// 1 像素 = 10 秒，缩到最粗的那一档。
pub const MIN_ZOOM: f32 = 0.1;
/// 标尺主刻度之间至少留这么多像素，太密就换下一档。
pub const MIN_LABEL_PX: f32 = 90.0;
/// 最细一档一帧占多少像素。帧刻度要看得清，同时标签还能分开。
const MAX_FRAME_PX: f32 = 8.0;

/// 可选的主刻度档位（秒）。
///
/// 最小从 1 秒起步：标签用的是整秒文本，半秒档会让同一个「0:01」出现两次。
/// 一秒以下的粒度交给帧刻度（[`visible_frames`]）。
pub const TICK_STEPS: &[f64] = &[
    1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0, 3600.0, 7200.0,
    14400.0,
];

/// 时间线的视口：横向 `scroll` 秒、每秒 `zoom` 像素，纵向 `track_scroll` 条轨道。
#[derive(Clone, Debug, PartialEq)]
pub struct ViewState {
    zoom: f32,
    scroll: f64,
    width: f32,
    /// 纵向可见的第一条轨道（0 起）。轨道多到一屏放不下时靠它换页。
    track_scroll: usize,
    /// 滚轮余量：触控板一次滑动拆成很多小增量，攒够一行高才走一行。
    track_wheel: f32,
    /// 刚加过轨道：下次布局同步时挪到最后一屏，让新轨立刻可见。
    reveal_last: bool,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            // 默认一秒八像素：一屏能看两分钟出头，先看清结构再放大。
            zoom: 8.0,
            scroll: 0.0,
            width: 800.0,
            track_scroll: 0,
            track_wheel: 0.0,
            reveal_last: false,
        }
    }
}

impl ViewState {
    pub fn new() -> Self {
        Self::default()
    }

    pub const fn zoom(&self) -> f32 {
        self.zoom
    }

    pub const fn scroll(&self) -> f64 {
        self.scroll
    }

    pub const fn width(&self) -> f32 {
        self.width
    }

    /// 最细一档：一帧占 [`MAX_FRAME_PX`] 个像素。帧率越高，每秒占的像素越多。
    pub fn max_zoom(fps: Rational) -> f32 {
        (fps.to_f64().clamp(1.0, 240.0) as f32) * MAX_FRAME_PX
    }

    /// 视口宽度变了（窗口缩放）。滚动位置按新宽度重新夹一遍。
    pub fn set_width(&mut self, width: f32, duration: Tick) {
        self.width = width.max(1.0);
        self.clamp(duration);
    }

    /// 设置缩放，供键盘 / 菜单用；鼠标缩放走 [`ViewState::zoom_around`]。
    pub fn set_zoom(&mut self, zoom: f32, max_zoom: f32, duration: Tick) {
        self.zoom = zoom.clamp(MIN_ZOOM, max_zoom.max(MIN_ZOOM));
        self.clamp(duration);
    }

    /// 视口左边界对应的时间（秒）。
    pub fn time_at(&self, x: f32) -> f64 {
        self.scroll + f64::from(x.max(0.0)) / f64::from(self.zoom)
    }

    /// 时间（秒）在视口里的横坐标（像素）。可能在视口外，由调用方裁掉。
    pub fn x_of(&self, seconds: f64) -> f32 {
        ((seconds - self.scroll) * f64::from(self.zoom)) as f32
    }

    pub fn visible_seconds(&self) -> f64 {
        f64::from(self.width) / f64::from(self.zoom)
    }

    /// 一个屏幕像素对应的时间（秒）。
    ///
    /// 鼠标落点只精确到像素，「回到刚才那个位置」的容差就取这个量级：
    /// 同一像素内换算出几十毫秒的差别是常态，不是用户放歪了。
    pub fn pixel_seconds(&self) -> f64 {
        1.0 / f64::from(self.zoom)
    }

    /// 横向滚动：正数往右（更晚），负数往左。
    pub fn scroll_by(&mut self, pixels: f32, duration: Tick) {
        self.scroll += f64::from(pixels) / f64::from(self.zoom);
        self.clamp(duration);
    }

    /// 直接跳到某个时间点；负数与超出范围都会被夹回。
    pub fn scroll_to(&mut self, seconds: f64, duration: Tick) {
        self.scroll = seconds;
        self.clamp(duration);
    }

    /// 以鼠标位置为锚点缩放：缩放前后，锚点下的时间点不动。
    pub fn zoom_around(&mut self, factor: f32, anchor_x: f32, max_zoom: f32, duration: Tick) {
        let anchor_time = self.time_at(anchor_x);
        self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, max_zoom.max(MIN_ZOOM));
        self.scroll = anchor_time - f64::from(anchor_x.max(0.0)) / f64::from(self.zoom);
        self.clamp(duration);
    }

    /// 把某个时间点放到视口中间（打开工程、定位播放头时用）。
    pub fn center_on(&mut self, seconds: f64, duration: Tick) {
        self.scroll = seconds - self.visible_seconds() * 0.5;
        self.clamp(duration);
    }

    /// 滚动边界：不能滚到 0 之前，也不能让序列尾部离开视口左边界。
    pub fn clamp(&mut self, duration: Tick) {
        let duration = duration.secs().max(0.0);
        let max_scroll = (duration - self.visible_seconds()).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max_scroll);
    }

    /// 一屏能画几条轨道时，最后一屏的第一行索引。
    fn max_track_scroll(total: usize, rows: usize) -> usize {
        total.saturating_sub(rows.max(1))
    }

    /// 这一屏该画哪些轨道。轨道被删、窗口变矮导致滚动值越界时，这里就地夹住。
    pub fn visible_tracks(&self, total: usize, rows: usize) -> Range<usize> {
        let rows = rows.max(1);
        let first = self.track_scroll.min(Self::max_track_scroll(total, rows));
        first..(first + rows).min(total)
    }

    /// 每次布局同步时夹一次滚动位置，并兑现「加轨后显示新轨」的请求。
    ///
    /// 加轨的入口拿不到窗口高度，所以只记一个意图；由 `sync_timeline_view` 带着
    /// 面板高度执行，不必把视口尺寸再塞进 app 状态。
    pub fn sync_tracks(&mut self, total: usize, rows: usize) {
        let rows = rows.max(1);
        if self.reveal_last {
            self.reveal_last = false;
            self.track_scroll = total.saturating_sub(rows);
        }
        self.track_scroll = self.track_scroll.min(Self::max_track_scroll(total, rows));
        if total <= rows {
            self.track_wheel = 0.0;
        }
    }

    /// 加轨后请下一次布局同步把视口挪到最后一屏。
    pub fn reveal_last_track(&mut self) {
        self.reveal_last = true;
    }

    /// 滚动条点选：直接跳到某一行开头。
    pub fn scroll_tracks_to(&mut self, first: usize, total: usize, rows: usize) {
        self.track_scroll = first.min(Self::max_track_scroll(total, rows.max(1)));
    }

    /// 轨道区滚轮：攒够一行高才走一行。GPUI 里上滚是正 y，取反才是「往回看」。
    /// 返回是否真的换了行，界面据此决定是否重绘。
    pub fn scroll_tracks_by_wheel(
        &mut self,
        pixels: f32,
        row_px: f32,
        total: usize,
        rows: usize,
    ) -> bool {
        let rows = rows.max(1);
        if total <= rows {
            self.track_wheel = 0.0;
            return false;
        }
        self.track_wheel += pixels;
        let step = row_px.max(1.0);
        let lines = (-self.track_wheel / step).trunc() as i32;
        if lines == 0 {
            return false;
        }
        self.track_wheel += lines as f32 * step;
        let before = self.track_scroll;
        let next = if lines >= 0 {
            before.saturating_add(lines as usize)
        } else {
            before.saturating_sub(lines.unsigned_abs() as usize)
        };
        self.track_scroll = next.min(Self::max_track_scroll(total, rows));
        before != self.track_scroll
    }

    /// 当前缩放下的主刻度间隔（秒）。
    pub fn tick_step(&self) -> f64 {
        tick_step(self.zoom)
    }
}

/// 选出主刻度间隔：第一个「在屏幕上至少占 `MIN_LABEL_PX`」的档位。
pub fn tick_step(zoom: f32) -> f64 {
    let zoom = f64::from(zoom.max(MIN_ZOOM));
    TICK_STEPS
        .iter()
        .copied()
        .find(|step| step * zoom >= f64::from(MIN_LABEL_PX))
        .unwrap_or_else(|| *TICK_STEPS.last().expect("档位表不能为空"))
}

/// 当前视口里要画的主刻度时间点。包含完全滚出左边界前的那一格，标签不会突然
/// 从边缘冒出来。
///
/// 右端按视口算而不是按序列时长算：序列只有 6 秒时，右边一整屏空白也该有刻度，
/// 否则播放头拖到 8 秒时标尺上连参照都没有。
pub fn visible_ticks(view: &ViewState) -> Vec<f64> {
    let step = view.tick_step();
    if step <= 0.0 {
        return Vec::new();
    }
    let end = view.scroll() + view.visible_seconds();
    let mut ticks = Vec::new();
    let mut at = (view.scroll() / step).floor() * step;
    while at <= end + step * 0.5 {
        if at >= 0.0 {
            ticks.push(at);
        }
        at += step;
    }
    ticks
}

/// 一帧在屏幕上占几个像素。放得够宽才铺帧刻度。
pub fn frame_pixels(view: &ViewState, fps: Rational) -> f64 {
    f64::from(view.zoom()) / fps.to_f64()
}

/// 视口里要画的帧序号（含两端）。
///
/// 右端和 [`visible_ticks`] 一样按视口算：序列末尾之后的空白也该有帧刻度，
/// 播放头拖出去时不至于失去参照。
pub fn visible_frames(view: &ViewState, fps: Rational) -> std::ops::RangeInclusive<i64> {
    let start = Tick::from_secs(view.scroll().max(0.0));
    let end = Tick::from_secs(view.scroll() + view.visible_seconds());
    let first = fps.floor_frame_of(start);
    let last = fps.ceil_frame_of(end).max(first);
    first..=last
}

/// 播放头能停到的最晚时间：视口可见多远就能走多远，序列末尾之后的空白也算。
///
/// 编辑时经常要先把播放头放到 8 秒、再导入下一段做出空档；夹在当前序列时长
/// （6 秒）上就永远排不出空档。滚动范围仍由 [`ViewState::clamp`] 管。
pub fn playhead_limit(view: &ViewState, duration: Tick) -> f64 {
    view.time_at(view.width()).max(duration.secs()).max(0.0)
}

/// 按住 Shift 拖动时的吸附：对齐到 0.01 秒网格。
///
/// 只用在鼠标输入这一个边界上；写回工程仍走 `Tick::from_secs` 的整数微秒。
pub fn snap_seconds(seconds: f64) -> f64 {
    if !seconds.is_finite() {
        return 0.0;
    }
    (seconds * 100.0).round() / 100.0
}

/// 标尺拖动一次的坐标换算：轨道区横坐标 → 序列时间（秒）。
///
/// 边界与吸附只在这里算一遍，界面层拿到结果就写 `Tick`。
pub fn seek_seconds(view: &ViewState, lane_x: f32, duration: Tick, snap: bool) -> f64 {
    let seconds = view
        .time_at(lane_x)
        .clamp(0.0, playhead_limit(view, duration));
    if snap { snap_seconds(seconds) } else { seconds }
}

/// 磁吸半径：播放头离目标这么近就贴上去（像素，缩放变化时手感一致）。
const MAGNET_PX: f32 = 6.0;

/// 把播放头落点吸到最近的标记 / 工作区端点上。
///
/// 鼠标定位只有像素精度，差一像素就是几十毫秒；没有这一步，「拖回刚才那个
/// 位置」全靠手稳，而 `Shift+M` / `Shift+I` 是否算「同一处」又依赖落点。
/// 只吸这些离散目标，不吸片段边界：放播放头去看某段中间不该被弹开。
pub fn snap_to_targets(view: &ViewState, at: Tick, targets: &[Tick]) -> Tick {
    let radius = f64::from(MAGNET_PX) / f64::from(view.zoom());
    let seconds = at.secs();
    let mut best = at;
    let mut best_distance = radius;
    for &target in targets {
        let distance = (target.secs() - seconds).abs();
        // 并列时留下后一个没关系：同一个像素里的两个目标本来就分不开。
        if distance <= best_distance {
            best_distance = distance;
            best = target;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dur(seconds: f64) -> Tick {
        Tick::from_secs(seconds)
    }

    #[test]
    fn 轨道窗口按可见行数裁切() {
        let mut view = ViewState::new();
        assert_eq!(view.visible_tracks(5, 2), 0..2);
        view.scroll_tracks_to(3, 5, 2);
        assert_eq!(view.visible_tracks(5, 2), 3..5, "最后一屏从第 4 条起");
        view.scroll_tracks_to(99, 5, 2);
        assert_eq!(view.visible_tracks(5, 2), 3..5, "跳转要夹在末尾");
        assert_eq!(view.visible_tracks(1, 2), 0..1, "没满一屏就只画这些");
        assert_eq!(view.visible_tracks(0, 2), 0..0);
    }

    #[test]
    fn 加轨后同步到最后一屏_删轨后夹回() {
        let mut view = ViewState::new();
        view.reveal_last_track();
        view.sync_tracks(4, 2);
        assert_eq!(view.visible_tracks(4, 2), 2..4, "新加的第 4 条要露出来");
        view.sync_tracks(2, 2);
        assert_eq!(view.visible_tracks(2, 2), 0..2, "删到只剩两条就回到顶部");
    }

    #[test]
    fn 滚轮攒够一行才换一行() {
        let mut view = ViewState::new();
        view.scroll_tracks_to(2, 6, 2);
        assert!(!view.scroll_tracks_by_wheel(20.0, 48.0, 6, 2));
        assert_eq!(view.visible_tracks(6, 2), 2..4, "20 像素还不够一行");
        // 上滚（正 y）回到前面的轨道：累计 80 像素走一行，余 32。
        assert!(view.scroll_tracks_by_wheel(60.0, 48.0, 6, 2));
        assert_eq!(view.visible_tracks(6, 2), 1..3);
        // 反向时先把上一段余量走完，不足一行不动。
        assert!(!view.scroll_tracks_by_wheel(-20.0, 48.0, 6, 2));
        assert_eq!(view.visible_tracks(6, 2), 1..3);
        assert!(view.scroll_tracks_by_wheel(-70.0, 48.0, 6, 2));
        assert_eq!(view.visible_tracks(6, 2), 2..4);
    }

    #[test]
    fn 缩放范围卡在八分之一帧与十秒之间() {
        let fps = Rational::FPS_25;
        let max = ViewState::max_zoom(fps);
        assert_eq!(max, 200.0, "25 帧/秒 × 每帧 8 像素");
        assert!((frame_pixels(&ViewState::new(), fps) - 8.0 / 25.0).abs() < 1e-9);

        let mut view = ViewState::new();
        view.set_zoom(1_000.0, max, dur(60.0));
        assert_eq!(view.zoom(), max, "不能比一帧 8 像素更细");

        view.set_zoom(0.0001, max, dur(60.0));
        assert_eq!(view.zoom(), MIN_ZOOM, "不能比十秒更粗");
    }

    #[test]
    fn 视口里只铺看得见的那几帧() {
        let fps = Rational::FPS_25;
        let mut view = ViewState::new();
        view.set_width(800.0, dur(600.0));
        // 200 像素/秒 → 一屏 4 秒，正好 100 帧。
        view.set_zoom(200.0, 200.0, dur(600.0));
        let frames = visible_frames(&view, fps);
        assert_eq!(*frames.start(), 0);
        assert_eq!(*frames.end(), 100);

        // 滚到 10 秒：从第 250 帧开始，不再是固定从 0 数。
        view.scroll_to(10.0, dur(600.0));
        let frames = visible_frames(&view, fps);
        assert_eq!(*frames.start(), 250);
        assert_eq!(*frames.end(), 350);
    }

    #[test]
    fn 播放头吸附到最近的标记() {
        // 默认 8 像素/秒，6 像素的磁吸半径就是 0.75 秒。
        let mut view = ViewState::new();
        let targets = [dur(3.0), dur(9.0)];

        assert_eq!(
            snap_to_targets(&view, Tick::from_millis(3300), &targets),
            dur(3.0),
            "离目标 0.3 秒，落在半径内"
        );
        assert_eq!(
            snap_to_targets(&view, Tick::from_millis(4000), &targets),
            Tick::from_millis(4000),
            "两边都超过半径就保持原位"
        );

        // 放大后半径按像素收缩：25 像素/秒时只剩 0.24 秒。
        view.set_zoom(25.0, 25.0, dur(60.0));
        assert_eq!(
            snap_to_targets(&view, Tick::from_millis(3200), &targets),
            dur(3.0)
        );
        assert_eq!(
            snap_to_targets(&view, Tick::from_millis(3300), &targets),
            Tick::from_millis(3300)
        );
    }

    #[test]
    fn 缩放以鼠标位置为锚点() {
        let mut view = ViewState::new();
        view.set_width(800.0, dur(600.0));
        view.scroll_by(100.0, dur(600.0)); // 右移 12.5 秒
        let anchor_x = 300.0;
        let before = view.time_at(anchor_x);

        view.zoom_around(2.0, anchor_x, 25.0, dur(600.0));
        assert_eq!(view.zoom(), 16.0);
        let after = view.time_at(anchor_x);
        assert!(
            (before - after).abs() < 1e-9,
            "锚点下的时间不该动：{before} → {after}"
        );
    }

    #[test]
    fn 滚动被时长与视口夹住() {
        let mut view = ViewState::new();
        view.set_width(800.0, dur(10.0)); // 8px/s 下一屏就能看完
        view.scroll_by(-100.0, dur(10.0));
        assert_eq!(view.scroll(), 0.0, "不能滚到负时间");
        view.scroll_by(10_000.0, dur(10.0));
        assert_eq!(view.scroll(), 0.0, "整条序列都在屏内时不该滚");

        view.set_zoom(0.1, 25.0, dur(10_000.0));
        view.scroll_by(1_000.0, dur(10_000.0));
        let expected = 10_000.0 - view.visible_seconds();
        assert!((view.scroll() - expected).abs() < 1e-9, "停在序列尾部");
    }

    #[test]
    fn 直接跳转滚动位置会被夹住() {
        let mut view = ViewState::new();
        // 8px/s：一屏 100 秒，序列 600 秒 → 最多滚到 500 秒。
        view.set_width(800.0, dur(600.0));

        view.scroll_to(-5.0, dur(600.0));
        assert_eq!(view.scroll(), 0.0, "不能滚到开始之前");

        view.scroll_to(10_000.0, dur(600.0));
        let expected = 600.0 - view.visible_seconds();
        assert!((view.scroll() - expected).abs() < 1e-9, "停在序列尾部");
    }

    #[test]
    fn 标尺随缩放换挡() {
        // 8px/s：至少 90px 一格 → 15 秒（120px）。
        assert_eq!(tick_step(8.0), 15.0);
        // 25px/s（1 像素 = 1 帧）：5 秒（125px）。
        assert_eq!(tick_step(25.0), 5.0);
        // 最细一档：1 秒一格（200px），标签不会退到半秒。
        assert_eq!(tick_step(200.0), 1.0);
        // 最粗一档：15 分钟（900s * 0.1 = 90px）。
        assert_eq!(tick_step(0.1), 900.0);
    }

    #[test]
    fn 刻度只覆盖可见范围() {
        let mut view = ViewState::new();
        view.set_width(400.0, dur(300.0)); // 50 秒一屏
        let ticks = visible_ticks(&view);
        assert_eq!(ticks.first(), Some(&0.0));
        assert!(ticks.iter().all(|tick| *tick <= 50.0 + 1e-9));
        assert_eq!(ticks.len(), 4, "8px/s 下每 15 秒一格：0/15/30/45");
    }

    #[test]
    fn 坐标换算可逆() {
        let mut view = ViewState::new();
        view.set_width(800.0, dur(600.0));
        view.zoom_around(1.5, 200.0, 25.0, dur(600.0));
        for x in [0.0, 123.0, 800.0] {
            let seconds = view.time_at(x);
            assert!((view.x_of(seconds) - x).abs() < 0.001);
        }
    }

    #[test]
    fn 定位播放头时居中() {
        let mut view = ViewState::new();
        view.set_width(800.0, dur(600.0));
        view.center_on(300.0, dur(600.0));
        assert!((view.scroll() - (300.0 - 50.0)).abs() < 1e-9);
        assert!((view.time_at(400.0) - 300.0).abs() < 1e-9, "中心点");
    }

    #[test]
    fn 短序列的标尺也画到视口右端() {
        let mut view = ViewState::new();
        view.set_width(800.0, dur(6.0)); // 8px/s：一屏 100 秒
        let ticks = visible_ticks(&view);
        assert_eq!(ticks.first(), Some(&0.0));
        assert!(
            ticks.iter().any(|tick| (*tick - 90.0).abs() < 1e-9),
            "6 秒之后也得有刻度"
        );
    }

    #[test]
    fn 播放头能停在序列末尾之后的空白里() {
        let mut view = ViewState::new();
        view.set_width(800.0, dur(6.0));
        assert!((playhead_limit(&view, dur(6.0)) - 100.0).abs() < 1e-9);
        let at = seek_seconds(&view, 64.0, dur(6.0), false);
        assert!((at - 8.0).abs() < 1e-9, "能拖到 8 秒：{at}");
        let far = seek_seconds(&view, 100_000.0, dur(6.0), false);
        assert!((far - 100.0).abs() < 1e-9, "但不许跑出视口：{far}");
    }

    #[test]
    fn shift_拖动时落点吸附到百分秒() {
        let view = ViewState::new();
        // 8px/s：64.03px ≈ 8.00375 秒，吸附后应是 8.00 秒。
        let raw = seek_seconds(&view, 64.03, dur(6.0), false);
        assert!((raw - 8.00375).abs() < 1e-6, "未吸附：{raw}");
        let snapped = seek_seconds(&view, 64.03, dur(6.0), true);
        assert!((snapped - 8.0).abs() < 1e-9, "吸附后：{snapped}");
        assert!((snap_seconds(8.006) - 8.01).abs() < 1e-9);
    }
}
