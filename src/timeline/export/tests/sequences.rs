//! 导出计划按序列 id 取，不跟着当前指针跑（ADR 0009）。

use super::*;
use crate::timeline::{ExportError, Rational, SequenceSpec};

fn spec() -> SequenceSpec {
    SequenceSpec::new(1920, 1080, Rational::FPS_30)
}

#[test]
fn 按_id_导的是那条序列而不是当前序列() {
    let scratch = Scratch::new("sequences-plan");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    let first = project.active_id();
    let second = project.add_sequence("空的", spec());
    let settings = ExportSettings::default();

    assert!(plan_sequence_id(&project, first, None, &settings).is_ok());
    assert_eq!(
        plan_sequence_id(&project, second, None, &settings),
        Err(ExportError::Empty),
        "空序列没有片段"
    );

    project.switch_sequence(second).unwrap();
    assert!(
        plan_sequence_id(&project, first, None, &settings).is_ok(),
        "切走之后仍按 id 导第一条"
    );
    assert!(
        plan_sequence_with(&project, None, &settings).is_err(),
        "当前序列是空的"
    );
}

#[test]
fn 序列被删后按_id_生成计划会明确报错() {
    let scratch = Scratch::new("sequences-gone");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    let first = project.active_id();
    let _ = project.add_sequence("B", spec());
    project.remove_sequence(first).expect("能删当前序列");

    assert_eq!(
        plan_sequence_id(&project, first, None, &ExportSettings::default()),
        Err(ExportError::SequenceGone)
    );
}

#[test]
fn 两条序列各有片段时按_id_生成的计划不同() {
    let scratch = Scratch::new("sequences-two");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    let first = project.active_id();
    let asset = project.assets[0].id;
    let second = project.add_sequence("B", spec());
    project.switch_sequence(second).unwrap();
    let track = project.sequence().tracks[0].id;
    let clip = project.sequence_mut().alloc_id();
    project
        .sequence_mut()
        .track_mut(track)
        .unwrap()
        .clips
        .push(ClipItem::new(clip, asset, s(0), s(5), Tick::ZERO));

    let a =
        plan_sequence_id(&project, first, None, &ExportSettings::default()).expect("第一条能导出");
    let b =
        plan_sequence_id(&project, second, None, &ExportSettings::default()).expect("第二条能导出");
    assert_ne!(a.filter, b.filter, "时长不同，滤镜链里的裁剪参数也不同");
    assert_eq!(a.inputs, b.inputs, "两条序列用的是同一个素材");
}
