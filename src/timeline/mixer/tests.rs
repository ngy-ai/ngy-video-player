use super::*;
use crate::timeline::{Id, TrackKind};

fn s(n: f64) -> Tick {
    Tick::from_secs(n)
}

fn track() -> Track {
    Track::new(Id::new(1), TrackKind::Audio, "A1")
}

#[test]
fn 三层音量连乘顺序固定() {
    let mut track = track();
    track.gain = 1.5;
    track.automation = vec![GainPoint::new(s(0.0), 0.5), GainPoint::new(s(4.0), 1.0)];
    // 片段 0.8 × 自动化 0.5（起点）× 推子 1.5
    assert!((linear_gain(&track, 0.8, s(0.0)) - 0.6).abs() < 1e-5);
    // 第二秒：自动化 0.625 → 0.8 × 0.625 × 1.5 = 0.75
    assert!((linear_gain(&track, 0.8, s(1.0)) - 0.75).abs() < 1e-5);
    // 没有自动化点就只剩片段 × 推子。
    track.automation.clear();
    assert!((linear_gain(&track, 0.8, s(1.0)) - 1.2).abs() < 1e-5);
}

#[test]
fn 坏值都退回原样而不是静音() {
    assert_eq!(clip_gain(f32::NAN), 1.0);
    assert_eq!(clip_gain(-2.0), 1.0);
    // 超过 +6 dB 夹到上限，不是丢弃。
    assert_eq!(clip_gain(10.0), MAX_GAIN);
    let mut track = track();
    track.gain = f32::INFINITY;
    assert_eq!(track.normalized_gain(), 1.0);
}

#[test]
fn 自动化在两个点之间线性() {
    let points = vec![GainPoint::new(s(0.0), 0.0), GainPoint::new(s(2.0), 1.0)];
    assert!((automation_at(&points, s(1.0)).unwrap() - 0.5).abs() < 1e-5);
    // 点外钳到端点（曲线之外的「线」不会外推）。
    assert_eq!(automation_at(&points, s(-5.0)), Some(0.0));
    assert_eq!(automation_at(&points, s(9.0)), Some(1.0));
    // 没有点：交给推子。
    assert_eq!(automation_at(&[], s(1.0)), None);
}

#[test]
fn 声像等功率且居中不衰减到零() {
    let (left, right) = pan_gains(0.0);
    assert!((left - right).abs() < 1e-6);
    // 居中约 -3 dB：1/√2。
    assert!((left - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
    // 全左 / 全右。
    let (left, right) = pan_gains(-1.0);
    assert!((left - 1.0).abs() < 1e-5 && right.abs() < 1e-6);
    let (left, right) = pan_gains(1.0);
    assert!(left.abs() < 1e-6 && (right - 1.0).abs() < 1e-5);
    // 平方和恒为 1：推声像不该顺手变响。
    for pan in [-0.75, -0.2, 0.0, 0.3, 0.9] {
        let (left, right) = pan_gains(pan);
        assert!(
            (left * left + right * right - 1.0).abs() < 1e-5,
            "pan {pan}"
        );
    }
    // 越界的值夹进 -1–1。
    assert_eq!(pan_gains(5.0), pan_gains(1.0));
}

#[test]
fn 自动化表达式按轨道时间而不是片段内时间() {
    let mut track = track();
    track.automation = vec![GainPoint::new(s(10.0), 1.0), GainPoint::new(s(12.0), 0.0)];
    // 片段从轨道第 10 秒开始：表达式里的分界点要落回 0 与 2。
    let expr = gain_expression(&track, s(10.0)).expect("有自动化");
    assert!(expr.contains("if(lt(t,2.0000)"), "{expr}");
    // 没有自动化就不插滤镜。
    track.automation.clear();
    assert_eq!(gain_expression(&track, s(0.0)), None);
    // 只有一个点：常量。
    track.automation = vec![GainPoint::new(s(3.0), 0.25)];
    assert_eq!(gain_expression(&track, s(0.0)).unwrap(), "0.250000");
}

#[test]
fn db_文案() {
    assert_eq!(db_label(1.0), "+0.0 dB");
    assert_eq!(db_label(0.5), "-6.0 dB");
    assert_eq!(db_label(0.0), "静音");
    assert_eq!(db_label(2.0), "+6.0 dB");
}
