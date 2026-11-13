//! 遮罩（M10-T12）在导出计划里的形状：alpha 图输入、`alphamerge`、黑底。
//!
//! 这里只钉字符串与输入列表，画面是不是真的被切掉由上层的 `--ignored` 端到端
//! 用例真渲一遍来看。

use super::*;
use crate::timeline::{Mask, MaskShape};

fn rect() -> Mask {
    Mask {
        enabled: true,
        shape: MaskShape::Rect {
            x: 0.25,
            y: 0.25,
            width: 0.5,
            height: 0.5,
        },
        feather: 0.0,
        offset: (0.0, 0.0),
    }
}

#[test]
fn 没有遮罩时计划里没有_alphamerge() {
    let scratch = Scratch::new("mask-none");
    let (project, _) = project_with(&scratch, &[(s(0), s(2), s(0))]);
    let plan = plan_sequence(&project).expect("能导出");

    assert!(plan.masks.is_empty());
    assert_eq!(plan.inputs.len(), 1, "只有素材本身");
    assert!(!plan.filter.contains("alphamerge"));
    assert!(!plan.filter.contains("yuva420p"));
    assert_eq!(plan.video_label, Some("[outv]".to_string()));
}

#[test]
fn 遮罩片段插进_alphamerge_并垫黑底() {
    let scratch = Scratch::new("mask-single");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(2), s(0))]);
    project.sequence_mut().tracks[0].clips[0].mask = rect();

    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);

    // 素材 + 一张 alpha 图；alpha 图占位在 inputs 里，任务层再换成真路径。
    assert_eq!(plan.inputs.len(), 2, "遮罩图也是输入");
    assert_eq!(plan.masks.len(), 1);
    assert_eq!(plan.masks[0].input, 1);
    assert!(plan.inputs[1].as_os_str().is_empty(), "计划里是占位路径");
    // PGM：4 字节头 + 每个像素一个字节。
    let (width, height) = (project.sequence().width, project.sequence().height);
    let header = format!("P5\n{width} {height}\n255\n");
    assert_eq!(
        plan.masks[0].pgm.len(),
        header.len() + (width * height) as usize
    );

    assert!(plan.filter.contains("alphamerge"), "{}", plan.filter);
    assert!(
        plan.filter.contains("format=yuva420p"),
        "带 alpha 的画面才能叠"
    );
    assert!(
        plan.filter.contains("color=c=black"),
        "透明区要落到黑底上：{}",
        plan.filter
    );
    assert_eq!(plan.video_label, Some("[outv]".to_string()));
}

#[test]
fn 上层遮罩把整条链改成带_alpha_再叠() {
    let scratch = Scratch::new("mask-two");
    let (mut project, ids) = project_with(&scratch, &[(s(0), s(2), s(0))]);
    // 第二条视频轨（列表靠下的那条是底）：上面那段带遮罩。
    let upper = project.sequence().tracks[0].id;
    project.sequence_mut().tracks[0].clips[0].mask = rect();
    let asset = project.sequence().tracks[0].clips[0].asset_id;
    let lower = project.sequence_mut().alloc_id();
    let mut track = crate::timeline::Track::new(lower, TrackKind::Video, "V2".to_string());
    let id = project.sequence_mut().alloc_id();
    track.clips.push(ClipItem::new(id, asset, s(0), s(2), s(0)));
    project.sequence_mut().tracks.push(track);
    assert_eq!(project.sequence().tracks[0].id, upper);

    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    assert_eq!(plan.masks.len(), 1, "只有上层有遮罩");
    assert!(plan.filter.contains("alphamerge"));
    // 两条轨各自 concat 成 layerv0 / layerv1，从黑底开始叠。
    assert!(plan.filter.contains("[layerv0]"), "{}", plan.filter);
    assert!(plan.filter.contains("[layerv1]"), "{}", plan.filter);
    assert!(plan.filter.contains("[maskbase]"), "{}", plan.filter);
    assert_eq!(plan.video_label, Some("[outv]".to_string()));
    assert_eq!(ids.len(), 1);
}

#[test]
fn 关掉的遮罩不插链() {
    let scratch = Scratch::new("mask-off");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(2), s(0))]);
    let mut mask = rect();
    mask.enabled = false;
    project.sequence_mut().tracks[0].clips[0].mask = mask;

    let plan = plan_sequence(&project).expect("能导出");
    assert!(plan.masks.is_empty());
    assert!(!plan.filter.contains("alphamerge"));
}
