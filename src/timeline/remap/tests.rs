use super::*;

#[test]
fn 没有曲线时就是常量倍率() {
    let curve = SpeedCurve::default();
    assert!(curve.is_empty());
    // 2 倍速：10 秒素材在时间线上只占 5 秒。
    assert!((curve.timeline_duration(10.0, 2.0).secs() - 5.0).abs() < 1e-6);
    // 0.5 倍速：占 20 秒。
    assert!((curve.timeline_duration(10.0, 0.5).secs() - 20.0).abs() < 1e-6);
    // 反解与正算互逆。
    assert!((curve.source_at_timeline(5.0, 10.0, 2.0) - 10.0).abs() < 1e-6);
}

#[test]
fn 倍率坏值退回原速() {
    assert_eq!(clamp_rate(f32::NAN), 1.0);
    assert_eq!(clamp_rate(0.0), 1.0);
    assert_eq!(clamp_rate(-3.0), 1.0);
    assert_eq!(clamp_rate(99.0), MAX_RATE);
    assert_eq!(clamp_rate(0.01), MIN_RATE);
}

#[test]
fn 两点之间线性插值() {
    let mut curve = SpeedCurve::default();
    curve.insert(0.0, 1.0);
    curve.insert(10.0, 2.0);
    assert!((curve.rate_at(5.0, 1.0) - 1.5).abs() < 1e-5);
    // 端点之外不外推。
    assert_eq!(curve.rate_at(-4.0, 1.0), 1.0);
    assert_eq!(curve.rate_at(99.0, 1.0), 2.0);
}

#[test]
fn 同一位置再记只改倍率() {
    let mut curve = SpeedCurve::default();
    assert!(curve.insert(1.0, 1.0));
    assert!(!curve.insert(1.0, 3.0));
    assert_eq!(curve.points.len(), 1);
    assert_eq!(curve.points[0].rate, 3.0);
}

#[test]
fn 加速段让时长变短且积分单调() {
    let mut curve = SpeedCurve::default();
    curve.insert(0.0, 1.0);
    curve.insert(10.0, 4.0);
    let duration = curve.timeline_duration(10.0, 1.0).secs();
    // 从 1x 线性加到 4x：一定比全程 1x（10 秒）短、比全程 4x（2.5 秒）长。
    assert!(duration < 10.0 && duration > 2.5, "{duration}");
    // 积分随时间单调：往前多放一秒素材，时间线只会更长。
    let mut previous = 0.0;
    for step in 1..=20 {
        let value = curve.timeline_duration(step as f64 * 0.5, 1.0).secs();
        assert!(value >= previous, "{value} < {previous}");
        previous = value;
    }
}

#[test]
fn 反解与正算互逆() {
    let mut curve = SpeedCurve::default();
    curve.insert(0.0, 0.5);
    curve.insert(4.0, 2.0);
    curve.insert(8.0, 1.0);
    let span = 8.0;
    let total = curve.timeline_duration(span, 1.0).secs();
    // 时间线走一半 → 素材位置应该在前半段里，且落在 (0, span)。
    let middle = curve.source_at_timeline(total * 0.5, span, 1.0);
    assert!(middle > 0.0 && middle < span, "{middle}");
    // 走完全程 → 素材到头。
    assert!((curve.source_at_timeline(total, span, 1.0) - span).abs() < 1e-6);
    // 半点前后单调。
    let later = curve.source_at_timeline(total * 0.75, span, 1.0);
    assert!(later >= middle);
}

#[test]
fn 空跨度算零而不是除零() {
    let curve = SpeedCurve::default();
    assert_eq!(curve.timeline_duration(0.0, 1.0), Tick::ZERO);
    assert_eq!(curve.timeline_duration(-1.0, 1.0), Tick::ZERO);
    assert_eq!(curve.source_at_timeline(1.0, 0.0, 1.0), 0.0);
}

#[test]
fn 累计时间表末项等于总时长() {
    let mut curve = SpeedCurve::default();
    curve.insert(0.0, 2.0);
    curve.insert(6.0, 0.5);
    let table = curve.cumulative(6.0, 1.0);
    assert_eq!(table.len(), STEPS + 1);
    assert_eq!(table[0], (0.0, 0.0));
    // 末项与 `timeline_duration` 是同一套积分：差在浮点精度内。
    let total = curve.timeline_duration(6.0, 1.0).secs();
    assert!((table[table.len() - 1].1 - total).abs() < 1e-6);
    // 素材位置与累计时间都单调不减。
    for pair in table.windows(2) {
        assert!(pair[1].0 >= pair[0].0);
        assert!(pair[1].1 >= pair[0].1);
    }
}

#[test]
fn setpts_表达式段数正确且末段回到总时长() {
    let mut curve = SpeedCurve::default();
    curve.insert(0.0, 1.0);
    curve.insert(4.0, 2.0);
    let expr = curve
        .setpts_expression(4.0, 1.0)
        .expect("有曲线就该有表达式");
    assert_eq!(expr.matches("if(lt(S,").count(), STEPS);
    let total = curve.timeline_duration(4.0, 1.0).secs();
    assert!(expr.contains(&format!(",{total:.6}")), "{expr}");
    // 没有曲线时不给表达式：导出侧照旧用常量倍率。
    assert_eq!(SpeedCurve::default().setpts_expression(4.0, 1.0), None);
}
