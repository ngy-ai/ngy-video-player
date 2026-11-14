//! 混音台面板的坐标换算：推子（dB 刻度）、声像条与自动化画布的比例互转。

use super::automation::{auto_at, auto_gain, auto_x, auto_y, normalize_automation};
use super::meter::{meter_level, overloaded, track_gain};
use super::*;
use crate::timeline::{Rational, Sequence, TrackKind};

#[test]
fn 推子零_db_在量程里的位置固定() {
    // dB 刻度下 0 dB 落在 (-60 .. +6.02) 的 60/66.02 ≈ 0.909 处。
    let ratio = ratio_from_gain(1.0);
    let expected = 60.0 / (60.0 + 20.0 * 2.0f32.log10());
    assert!((ratio - expected).abs() < 0.01, "{ratio}");
    // 来回一趟不掉值。
    assert!((gain_from_ratio(ratio) - 1.0).abs() < 1e-3);
}

#[test]
fn 推子两端是静音与上限() {
    assert_eq!(gain_from_ratio(0.0), 0.0);
    assert_eq!(gain_from_ratio(0.005), 0.0, "最左一小段是静音死区");
    assert!((gain_from_ratio(1.0) - mixer::MAX_GAIN).abs() < 1e-3);
    assert_eq!(ratio_from_gain(0.0), 0.0, "静音的条在最左");
    // 越界值不 panic，夹在两端。
    assert_eq!(gain_from_ratio(-3.0), 0.0);
    assert!((gain_from_ratio(9.0) - mixer::MAX_GAIN).abs() < 1e-3);
}

#[test]
fn 推子是_db_刻度不是线性() {
    // -6 dB（0.5012）在条上的位置应当比线性刻度（0.25）靠右得多。
    let ratio = ratio_from_gain(0.5012);
    assert!(ratio > 0.6, "-6 dB 落在 {ratio}");
    // 相邻两档在条上是等距的：-12 与 -6 的间距等于 -6 与 0 的间距。
    let gap_low = ratio_from_gain(0.5012) - ratio_from_gain(0.2512);
    let gap_high = ratio_from_gain(1.0) - ratio_from_gain(0.5012);
    assert!((gap_low - gap_high).abs() < 1e-3);
}

#[test]
fn 声像条中间是居中() {
    assert_eq!(pan_label(pan_from_ratio(0.5)), "居中");
    assert!((pan_from_ratio(0.0) + 1.0).abs() < 1e-6, "最左全左");
    assert!((pan_from_ratio(1.0) - 1.0).abs() < 1e-6, "最右全右");
    // 来回一趟。
    for pan in [-1.0, -0.5, 0.0, 0.25, 1.0] {
        let back = pan_from_ratio(ratio_from_pan(pan));
        assert!((back - pan).abs() < 1e-6, "{pan} → {back}");
    }
    assert_eq!(pan_label(-0.5), "L50");
    assert_eq!(pan_label(0.3), "R30");
    // 越界的声像按夹取后的值显示。
    assert_eq!(pan_label(5.0), "R100");
}

#[test]
fn 自动化画布横轴按序列时长往返() {
    let duration = Tick::from_secs(8.0);
    for (at, expected) in [
        (Tick::ZERO, 0.0),
        (Tick::from_secs(2.0), 0.25),
        (Tick::from_secs(8.0), 1.0),
    ] {
        let x = auto_x(at, duration);
        assert!((x - expected).abs() < 1e-5, "{at:?} → {x}");
        assert_eq!(auto_at(x, duration), at);
    }
    assert_eq!(auto_x(Tick::from_secs(20.0), duration), 1.0, "拖出右边界");
    assert_eq!(
        auto_x(Tick::from_secs(1.0), Tick::ZERO),
        0.0,
        "空序列不除零"
    );
    assert_eq!(auto_at(-1.0, duration), Tick::ZERO);
}

#[test]
fn 自动化画布纵轴与推子同一套_db_刻度() {
    assert_eq!(auto_y(0.0), 1.0, "静音在画布最底");
    assert_eq!(auto_y(mixer::MAX_GAIN), 0.0, "上限在画布最顶");
    let y = auto_y(1.0);
    assert!(y > 0.0 && y < 1.0, "0 dB 在中间偏上：{y}");
    assert!((auto_gain(y) - 1.0).abs() < 1e-3, "来回一趟回到 0 dB");
    assert_eq!(auto_gain(1.0), 0.0, "拖到最底是静音档");
    assert!((auto_gain(0.0) - mixer::MAX_GAIN).abs() < 1e-3);
}

#[test]
fn 写回前会钳制排序并去掉同刻重复点() {
    let mut points = vec![
        GainPoint::new(Tick::from_secs(3.0), 9.0),
        GainPoint::new(Tick::from_secs(1.0), -2.0),
        GainPoint::new(Tick::from_micros(1_000_500), 1.5),
        GainPoint::new(Tick::from_micros(-500_000), 0.5),
    ];
    normalize_automation(&mut points);
    assert_eq!(points.len(), 3, "1s 与 1.0005s 只留一个：{points:?}");
    assert_eq!(points[0].at, Tick::ZERO, "负起点夹到 0");
    assert!((points[1].gain - 1.5).abs() < 1e-5, "同刻留后来的点");
    assert_eq!(points[2].gain, mixer::MAX_GAIN, "超上限夹到 MAX_GAIN");
}

#[test]
fn 有效增益条与推子同刻度且能标出过载() {
    assert_eq!(meter_level(0.0), 0.0, "静音应该是空条");
    assert_eq!(meter_level(-1.0), 0.0);
    assert_eq!(meter_level(f32::NAN), 0.0, "坏值不能画出 NaN 高度");
    let unity = meter_level(1.0);
    assert!((unity - ratio_from_gain(1.0)).abs() < 1e-6);
    assert!(meter_level(2.0) > unity, "增益越高条越长");
    assert!(!overloaded(1.0), "正好 0 dB 不算过载");
    assert!(overloaded(1.5));
    assert!(!overloaded(0.0));
    assert!(!overloaded(f32::NAN));
}

#[test]
fn 空轨不发声所以增益条是零() {
    let mut sequence = Sequence::new("meter", Rational::FPS_30, 320, 240);
    let track = sequence.add_track(TrackKind::Audio);
    assert_eq!(track_gain(&sequence, track, Tick::from_secs(1.0)), 0.0);
}
