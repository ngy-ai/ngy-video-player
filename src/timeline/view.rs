//! 时间线视图状态：缩放、滚动与「秒 ↔ 像素」换算。
//!
//! 纯数字，不碰 GPUI：界面把视口宽度和鼠标位置传进来，这里只回答「第几秒在哪个
//! 像素」「滚一格走多远」「标尺该用多大刻度」。三小时工程的滚动边界和缩放锚点
//! 因此可以用单测钉住，不必开窗口反复试。
//!
//! 缩放范围按 `TODO.md` M2-T01 固定：最粗 `1 像素 = 10 秒`（[`MIN_ZOOM`]），
//! 最细 `1 像素 = 1 帧`（[`ViewState::max_zoom`]，即每秒帧率那么多像素）。

use crate::timeline::{Rational, Tick};

/// 1 像素 = 10 秒，缩到最粗的那一档。
pub const MIN_ZOOM: f32 = 0.1;
/// 标尺主刻度之间至少留这么多像素，太密就换下一档。
pub const MIN_LABEL_PX: f32 = 90.0;

/// 可选的主刻度档位（秒）。从一帧到四小时，够覆盖「1 像素 = 1 帧」到
/// 「1 像素 = 10 秒」之间的所有缩放级别。
pub const TICK_STEPS: &[f64] = &[
    0.04, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1800.0,
    3600.0, 7200.0, 14400.0,
];

/// 时间线的视口：左边界 `scroll` 秒，每秒 `zoom` 像素，宽 `width` 像素。
#[derive(Clone, Debug, PartialEq)]
pub struct ViewState {
    zoom: f32,
    scroll: f64,
    width: f32,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            // 默认一秒八像素：一屏能看两分钟出头，先看清结构再放大。
            zoom: 8.0,
            scroll: 0.0,
            width: 800.0,
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

    /// 最细一档：1 像素 = 1 帧。帧率越高，每秒占的像素越多。
    pub fn max_zoom(fps: Rational) -> f32 {
        (fps.to_f64().clamp(1.0, 240.0)) as f32
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
pub fn visible_ticks(view: &ViewState, duration: Tick) -> Vec<f64> {
    let step = view.tick_step();
    if step <= 0.0 {
        return Vec::new();
    }
    let end = (view.scroll() + view.visible_seconds()).min(duration.secs().max(0.0));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn dur(seconds: f64) -> Tick {
        Tick::from_secs(seconds)
    }

    #[test]
    fn 缩放范围卡在一帧与十秒之间() {
        let fps = Rational::FPS_25;
        let max = ViewState::max_zoom(fps);
        assert_eq!(max, 25.0, "1 像素 = 1 帧");

        let mut view = ViewState::new();
        view.set_zoom(1_000.0, max, dur(60.0));
        assert_eq!(view.zoom(), max, "不能比一帧更细");

        view.set_zoom(0.0001, max, dur(60.0));
        assert_eq!(view.zoom(), MIN_ZOOM, "不能比十秒更粗");
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
        // 最粗一档：15 分钟（900s * 0.1 = 90px）。
        assert_eq!(tick_step(0.1), 900.0);
    }

    #[test]
    fn 刻度只覆盖可见范围() {
        let mut view = ViewState::new();
        view.set_width(400.0, dur(300.0)); // 50 秒一屏
        let ticks = visible_ticks(&view, dur(300.0));
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
}
