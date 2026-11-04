//! 小窗纯计算的单元测试：不需要 ffmpeg，也不需要绘制环境。

use super::super::MiniState;
use super::geometry::{mini_aspect, mini_width, time_for_x, volume_for_y};
use super::*;

/// 构造一块轨道区域，省得每处都写 `Bounds::new(point(..), size(..))`。
fn track(origin: (f32, f32), extent: (f32, f32)) -> Bounds<Pixels> {
    Bounds::new(
        point(px(origin.0), px(origin.1)),
        size(px(extent.0), px(extent.1)),
    )
}

#[test]
fn 小窗宽度落在上下限内并给小屏留出余量() {
    assert_eq!(mini_width(400.0, 1920.0), 400.0);
    assert_eq!(mini_width(100.0, 1920.0), MINI_MIN_WIDTH);
    assert_eq!(mini_width(1000.0, 1920.0), MINI_MAX_WIDTH);
    // 800 * 40% = 320，正好是下限；再宽的偏好也放不下。
    assert_eq!(mini_width(560.0, 800.0), MINI_MIN_WIDTH);
    // 极小的屏幕上宁可贴着下限，也不能算出比下限还窄的窗口。
    assert_eq!(mini_width(560.0, 200.0), MINI_MIN_WIDTH);
}

#[test]
fn 小窗高宽比跟随旋转并夹在可用范围() {
    let wide = 16.0 / 9.0;
    assert!((mini_aspect(wide, false) - wide).abs() < 1e-6);
    assert!((mini_aspect(wide, true) - wide.recip()).abs() < 1e-6);
    assert_eq!(mini_aspect(10.0, false), 4.0);
    assert_eq!(mini_aspect(0.1, false), 0.4);
}

#[test]
fn 进度条坐标换算() {
    let bounds = track((100.0, 0.0), (200.0, 14.0));
    assert_eq!(time_for_x(bounds, px(100.0), 100.0), Some(0.0));
    assert_eq!(time_for_x(bounds, px(200.0), 100.0), Some(50.0));
    assert_eq!(time_for_x(bounds, px(300.0), 100.0), Some(100.0));
    // 指针跑到轨道外按端点算，不能给出负进度或超出时长。
    assert_eq!(time_for_x(bounds, px(0.0), 100.0), Some(0.0));
    assert_eq!(time_for_x(bounds, px(999.0), 100.0), Some(100.0));
    // 时长未知、轨道还没量出来时交给调用方保持现状。
    assert_eq!(time_for_x(bounds, px(200.0), 0.0), None);
    assert_eq!(
        time_for_x(track((0.0, 0.0), (0.0, 14.0)), px(0.0), 100.0),
        None
    );
}

#[test]
fn 音量滑块坐标换算() {
    let bounds = track((0.0, 10.0), (20.0, 100.0));
    // 竖直滑块「上大下小」：轨道顶端是 130%，底端是 0。
    assert_eq!(volume_for_y(bounds, px(10.0)), Some(130.0));
    assert_eq!(volume_for_y(bounds, px(60.0)), Some(65.0));
    assert_eq!(volume_for_y(bounds, px(110.0)), Some(0.0));
    assert_eq!(volume_for_y(bounds, px(0.0)), Some(130.0));
    assert_eq!(volume_for_y(bounds, px(999.0)), Some(0.0));
    assert_eq!(volume_for_y(track((0.0, 0.0), (20.0, 0.0)), px(0.0)), None);
}

#[test]
fn 小窗状态只在可见内容变化时重绘() {
    let base = MiniState {
        title: SharedString::from("片子"),
        position: 12.0,
        duration: 100.0,
        playing: true,
        volume: 80.0,
        muted: false,
        empty: false,
        items: 3,
    };
    assert!(base.same_as(&base.clone()));

    // 百分之一以内的进度 / 音量抖动不算变化。
    let mut nudged = base.clone();
    nudged.position = 12.004;
    nudged.volume = 80.004;
    assert!(nudged.same_as(&base));

    // 超过阈值、或者别的字段变了就要重绘。
    let mut moved = base.clone();
    moved.position = 12.01;
    assert!(!moved.same_as(&base));

    let mut playing = base.clone();
    playing.playing = false;
    assert!(!playing.same_as(&base));

    let mut listed = base.clone();
    listed.items = 4;
    assert!(!listed.same_as(&base));
}
