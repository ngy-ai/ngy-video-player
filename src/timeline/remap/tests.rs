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

#[test]
fn 速度曲线挂在片段上能存盘读回() {
    let mut project = crate::timeline::Project::default();
    let id = project.add_asset(std::path::Path::new("C:/素材/b.mp4"));
    let mut sequence =
        crate::timeline::Sequence::new("序列", crate::timeline::Rational::FPS_25, 1920, 1080);
    sequence.tracks.clear();
    let track_id = sequence.add_track(crate::timeline::TrackKind::Video);
    let clip_id = sequence.alloc_id();
    let mut clip = crate::timeline::ClipItem::new(
        clip_id,
        id,
        crate::timeline::Tick::ZERO,
        crate::timeline::Tick::from_secs(8.0),
        crate::timeline::Tick::ZERO,
    );
    clip.remap.insert(0.0, 2.0);
    clip.remap.insert(4.0, 0.5);
    if let Some(track) = sequence.track_mut(track_id) {
        track.clips.push(clip);
    }
    let sequence_id = sequence.id;
    project.sequences = vec![sequence];
    project.active_sequence = sequence_id;

    let path = std::env::temp_dir().join("ngy-remap.ngyproj");
    project.save(&path).expect("保存");
    let opened = crate::timeline::Project::open(&path).expect("打开");
    let back = opened.sequence().clip(clip_id).expect("片段还在");
    assert_eq!(back.remap.points.len(), 2);
    assert_eq!(back.remap.rate_at(0.0, 1.0), 2.0);
    assert_eq!(back.remap.rate_at(4.0, 1.0), 0.5);
    // 曲线非空时 `duration()` 走曲线的积分，不再是常量倍率。
    assert!(back.has_remap());
    let expected = back
        .remap
        .timeline_duration((back.source_out - back.source_in).secs(), back.rate());
    assert_eq!(back.duration(), expected + back.hold);
    assert_ne!(
        back.duration(),
        crate::timeline::Tick::from_secs(
            (back.source_out - back.source_in).secs() / f64::from(back.rate())
        ),
        "挂上曲线之后时长必须跟着积分走"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn 片段时长与素材时刻都跟着曲线走() {
    let mut clip = crate::timeline::ClipItem::new(
        crate::timeline::Id::new(1),
        crate::timeline::AssetId::new(1),
        crate::timeline::Tick::ZERO,
        crate::timeline::Tick::from_secs(4.0),
        crate::timeline::Tick::from_secs(10.0),
    );
    // 从 2x 线性降到 0.5x：积分是 4·ln(4)/1.5 ≈ 3.695 秒，比「平均 1.25x」的
    // 3.2 秒长——用算术平均会算短，所以这里对的是积分本身。
    clip.remap.insert(0.0, 2.0);
    clip.remap.insert(4.0, 0.5);

    let motion = clip.motion().secs();
    let integral = clip.remap.timeline_duration(4.0, 1.0).secs();
    assert!(
        (motion - integral).abs() < 1e-3,
        "运动时长 {motion} ≠ {integral}"
    );
    assert!((motion - 3.695).abs() < 0.02, "运动时长 {motion}");
    assert_eq!(clip.end(), clip.start + clip.motion() + clip.hold);
    assert_eq!(clip.duration(), clip.motion(), "没有定格时时长就是运动时长");

    // 时间线推进，素材位置单调跟着走；起点在素材入点、末尾贴到出点。
    let mut previous = -1.0;
    for step in 0..10 {
        let at = clip.start + Tick::from_secs(motion * f64::from(step) / 10.0);
        let source = clip.source_at(at).expect("落在片段内").secs();
        assert!(source >= previous, "{source} < {previous}");
        previous = source;
    }
    assert!(clip.source_at(clip.start).unwrap().secs() < 0.001);
    let last = clip.source_at(clip.end() - Tick::from_micros(1)).unwrap();
    assert!(last.secs() > 3.9, "末帧应当贴着出点：{}", last.secs());

    // 正反解互逆：素材 3 秒 → 时间线 → 素材，误差在十分之一秒内。
    let back = clip
        .timeline_at(clip.source_in + Tick::from_secs(3.0))
        .expect("素材在片段内");
    let round = clip.source_at(back).expect("时间线落在片段内");
    assert!(
        (round.secs() - 3.0).abs() < 0.1,
        "往返之后是 {} 秒",
        round.secs()
    );

    // 没有曲线的片段完全不受影响。
    let mut plain = clip.clone();
    plain.remap = SpeedCurve::default();
    assert!(!plain.has_remap());
    assert_eq!(plain.motion(), Tick::from_secs(4.0));
    assert_eq!(
        plain.source_at(plain.start + Tick::from_secs(1.0)),
        Some(Tick::from_secs(1.0))
    );
}
