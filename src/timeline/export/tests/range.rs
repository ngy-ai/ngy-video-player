//! 区间导出（M9-T13）：入出点之间只导这一段，成片重新从 0 起算。
//!
//! 做法是在拼好的成片上再切一刀，所以这里钉的是「切点与夹取」：
//! 序列内的范围原样写进 `trim` / `atrim`，超出序列的夹到结尾，夹空了就报错。

use super::*;

#[test]
fn 只导工作区() {
    let scratch = Scratch::new("range");
    let (project, _) = project_with(&scratch, &[(s(0), s(10), Tick::ZERO)]);

    let plan = plan_sequence_range(&project, Some((s(3), s(5)))).expect("能导出");
    assert_connected(&plan.filter);
    assert!(
        plan.filter
            .contains("[outv]trim=start=3.000000:end=5.000000,setpts=PTS-STARTPTS[rangev]"),
        "{}",
        plan.filter
    );
    assert_eq!(plan.video_label.as_deref(), Some("[rangev]"));
    // 只切一刀：`[outv]` 应当只被 trim 消费一次。
    assert_eq!(
        plan.filter.matches("[outv]trim=").count(),
        1,
        "{}",
        plan.filter
    );
}

#[test]
fn 音频也跟着切并重新起算() {
    let scratch = Scratch::new("range-audio");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(10), Tick::ZERO)]);
    let asset = project.assets[0].id;
    let audio = project.sequence.tracks[1].id;
    let id = project.sequence.alloc_id();
    project
        .sequence
        .track_mut(audio)
        .expect("音频轨存在")
        .clips
        .push(ClipItem::new(id, asset, s(0), s(10), Tick::ZERO));

    let plan = plan_sequence_range(&project, Some((s(2), s(4)))).expect("能导出");
    assert_connected(&plan.filter);
    assert!(
        plan.filter
            .contains("[outa]atrim=start=2.000000:end=4.000000,asetpts=PTS-STARTPTS[rangea]"),
        "{}",
        plan.filter
    );
    assert_eq!(plan.audio_label.as_deref(), Some("[rangea]"));
}

#[test]
fn 工作区超出序列时夹到结尾() {
    let scratch = Scratch::new("range-clamp");
    let (project, _) = project_with(&scratch, &[(s(0), s(10), Tick::ZERO)]);

    let plan = plan_sequence_range(&project, Some((s(8), s(30)))).expect("能导出");
    assert!(
        plan.filter.contains("trim=start=8.000000:end=10.000000"),
        "{}",
        plan.filter
    );

    // 入点比序列还靠后：夹完是 10–10，等于空。
    assert_eq!(
        plan_sequence_range(&project, Some((s(12), s(30)))).unwrap_err(),
        ExportError::EmptyRange
    );
}

#[test]
fn 空工作区拒绝导出() {
    let scratch = Scratch::new("range-empty");
    let (project, _) = project_with(&scratch, &[(s(0), s(10), Tick::ZERO)]);

    assert_eq!(
        plan_sequence_range(&project, Some((s(5), s(5)))).unwrap_err(),
        ExportError::EmptyRange,
        "入点等于出点没有内容"
    );
    assert_eq!(
        plan_sequence_range(&project, Some((s(6), s(2)))).unwrap_err(),
        ExportError::EmptyRange,
        "反向区间不许导"
    );
}

#[test]
fn 没有工作区时照旧导整条() {
    let scratch = Scratch::new("range-none");
    let (project, _) = project_with(&scratch, &[(s(0), s(10), Tick::ZERO)]);

    let whole = plan_sequence_range(&project, None).expect("能导出");
    assert_eq!(whole, plan_sequence(&project).expect("能导出"));
    assert!(!whole.filter.contains("rangev"));
}
