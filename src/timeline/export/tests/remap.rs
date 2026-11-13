//! 速度曲线（时间重映射）在导出计划里的形状：`setpts` 表达式、音频分段、不接转场。

use super::*;

/// 一条曲线：前 2 秒 2x、后 2 秒 0.5x（素材跨度 4 秒）。
fn curve(span: f64) -> crate::timeline::SpeedCurve {
    let mut curve = crate::timeline::SpeedCurve::default();
    curve.insert(0.0, 2.0);
    curve.insert(span / 2.0, 2.0);
    curve.insert(span, 0.5);
    curve
}

#[test]
fn 速度曲线换成setpts表达式() {
    let scratch = Scratch::new("remap-setpts");
    let (mut project, ids) = project_with(&scratch, &[(s(0), s(4), Tick::ZERO)]);
    let clip = ids[0];
    crate::timeline::set_remap(project.sequence_mut(), clip, &curve(4.0));

    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    assert!(
        plan.filter.contains("setpts='("),
        "曲线要落成表达式而不是常量倍率：{}",
        plan.filter
    );
    assert!(
        plan.filter.contains("TB"),
        "表达式必须换算回时基单位：{}",
        plan.filter
    );
    assert!(
        !plan.filter.contains("setpts=PTS-STARTPTS"),
        "有曲线时不能再用零位移的 setpts：{}",
        plan.filter
    );
    // 时间线时长也要跟着积分走，否则 concat 会按旧长度截断。
    assert!(
        project.sequence().duration() < s(4),
        "2x → 0.5x 的片段应当比素材短：{:?}",
        project.sequence().duration()
    );
}

#[test]
fn 没有曲线的片段仍然用常量倍率() {
    let scratch = Scratch::new("remap-plain");
    let (mut project, ids) = project_with(&scratch, &[(s(0), s(4), Tick::ZERO)]);
    // 只改常量倍率：setpts 走短写法，不该冒出表达式。
    crate::timeline::set_speed(project.sequence_mut(), ids[0], 2.0);

    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    assert!(plan.filter.contains("setpts=(PTS-STARTPTS)/2.000000"));
    assert!(!plan.filter.contains("TB"), "{}", plan.filter);
}

#[test]
fn 曲线片段不接转场() {
    let scratch = Scratch::new("remap-transition");
    // 两段紧挨着，接缝上的转场本来会插 xfade；后一段挂了曲线就得退化成硬切。
    let (mut project, ids) = project_with(&scratch, &[(s(0), s(2), s(0)), (s(0), s(2), s(2))]);
    let track = project.sequence().tracks[0].id;
    let transition = project.sequence_mut().alloc_id();
    project
        .sequence_mut()
        .track_mut(track)
        .expect("视频轨存在")
        .transitions
        .push(crate::timeline::Transition::new(
            transition,
            ids[0],
            ids[1],
            crate::timeline::TransitionKind::CrossDissolve,
            s(1),
        ));
    crate::timeline::set_remap(project.sequence_mut(), ids[1], &curve(2.0));

    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    assert!(
        !plan.filter.contains("xfade"),
        "曲线片段的时长按积分算，多取的素材落点对不上，必须退化成硬切：{}",
        plan.filter
    );
}

#[test]
fn 音频按曲线折点分段变速() {
    let scratch = Scratch::new("remap-audio");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    let asset = project.assets[0].id;
    let audio = add_track_clip(
        &mut project,
        TrackKind::Audio,
        asset,
        s(0),
        s(4),
        Tick::ZERO,
    );
    // 三个折点 → 三段：2x → 0.5x → 2x。
    let mut curve = crate::timeline::SpeedCurve::default();
    curve.insert(0.0, 2.0);
    curve.insert(1.0, 0.5);
    curve.insert(3.0, 2.0);
    crate::timeline::set_remap(project.sequence_mut(), audio, &curve);

    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    assert!(
        plan.filter.contains("asplit=3"),
        "同一路声音切成三段，先要拆流：{}",
        plan.filter
    );
    assert!(
        plan.filter.contains("concat=n=3:v=0:a=1"),
        "分段变速之后要拼回一条：{}",
        plan.filter
    );
    // 两段斜坡用的是调和倍率 (0.5-2)/ln(0.25) ≈ 1.082（不是算术平均的 1.25），
    // 末尾那段是纯 2x。
    assert!(
        plan.filter.contains("atempo=1.082021"),
        "斜坡段的调和倍率：{}",
        plan.filter
    );
    assert!(
        plan.filter.contains("atempo=2.000000"),
        "恒定段照旧：{}",
        plan.filter
    );
    assert_eq!(
        plan.filter.matches("concat=n=1").count(),
        1,
        "只有整条轨那次 concat 是 n=1：{}",
        plan.filter
    );
}

#[test]
fn 单点曲线退回一条恒定倍率() {
    let scratch = Scratch::new("remap-single");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    let asset = project.assets[0].id;
    let audio = add_track_clip(
        &mut project,
        TrackKind::Audio,
        asset,
        s(0),
        s(4),
        Tick::ZERO,
    );
    let mut curve = crate::timeline::SpeedCurve::default();
    curve.insert(0.0, 2.0);
    crate::timeline::set_remap(project.sequence_mut(), audio, &curve);

    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    assert!(!plan.filter.contains("asplit="), "{}", plan.filter);
    assert!(plan.filter.contains("atempo=2.000000"), "{}", plan.filter);
}
