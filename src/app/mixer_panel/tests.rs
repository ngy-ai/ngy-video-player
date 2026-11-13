//! 混音台面板的坐标换算：推子（dB 刻度）与声像条的比例互转。

use super::*;

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
