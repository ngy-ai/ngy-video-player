//! 字幕进导出计划的形状：ASS 文本、烧录滤镜、软字幕输入与工作区对齐。

use std::path::Path;

use super::*;
use crate::timeline::{SubtitleItem, SubtitlePosition};

fn with_subtitle(scratch: &Scratch) -> Project {
    let (mut project, _) = project_with(scratch, &[(s(0), s(10), Tick::ZERO)]);
    let track = project.sequence_mut().add_track(TrackKind::Subtitle);
    let id = project.sequence_mut().alloc_id();
    let mut entry = SubtitleItem::new(id, s(5), s(8), "字幕内容");
    entry.style.position = SubtitlePosition::Top;
    project
        .sequence_mut()
        .track_mut(track)
        .unwrap()
        .subtitles
        .push(entry);
    project
}

#[test]
fn 没有字幕时计划不带附件() {
    let scratch = Scratch::new("subtitle-none");
    let (project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    let plan = plan_sequence(&project).expect("能导出");
    assert!(plan.subtitle.is_none());
}

#[test]
fn 字幕文本进计划而原有命令不变() {
    let scratch = Scratch::new("subtitle-plan");
    let project = with_subtitle(&scratch);
    let plan = plan_sequence(&project).expect("能导出");
    let attachment = plan.subtitle.as_ref().expect("带上字幕附件");
    assert_eq!(attachment.mode, SubtitleMode::Burn, "默认烧录");
    assert!(attachment.ass.contains("字幕内容"));
    assert!(attachment.ass.contains("Dialogue: 0,0:00:05.00,0:00:08.00"));
    assert!(attachment.ass.contains(",1,2,1,8,20,20,20,1"), "位置=上");

    // 计划本身仍是纯数据：args() 不含任何字幕参数，老的字符串快照继续成立。
    let args = plan.args();
    assert!(!args.iter().any(|arg| arg.contains("subtitles=")));
    assert!(!args.iter().any(|arg| arg == "-c:s"));
    assert_eq!(
        args.iter().filter(|arg| *arg == "-i").count(),
        plan.inputs.len()
    );
    assert_connected(&plan.filter);
}

#[test]
fn 隐藏的字幕轨不进计划() {
    let scratch = Scratch::new("subtitle-hidden");
    let mut project = with_subtitle(&scratch);
    let track = project
        .sequence()
        .tracks
        .iter()
        .find(|track| track.kind == TrackKind::Subtitle)
        .unwrap()
        .id;
    project.sequence_mut().track_mut(track).unwrap().muted = true;
    assert!(plan_sequence(&project).unwrap().subtitle.is_none());
}

#[test]
fn 烧录把字幕滤镜接在成片之后() {
    let scratch = Scratch::new("subtitle-burn");
    let plan = plan_sequence(&with_subtitle(&scratch)).expect("能导出");
    let path = Path::new(r"C:\temp\字幕.ass");
    let args = plan.args_with_subtitle(path, SubtitleMode::Burn, "mov_text");
    let filter_index = args
        .iter()
        .position(|arg| arg == "-filter_complex")
        .unwrap();
    let filter = &args[filter_index + 1];
    assert!(
        filter.ends_with("[outv]subtitles=filename='C\\:/temp/字幕.ass'[subv]"),
        "{filter}"
    );
    assert!(
        args.iter().any(|arg| arg == "[subv]"),
        "视频 map 改到烧录后的标签"
    );
    assert!(!args.iter().any(|arg| arg == "[outv]"));
    assert_eq!(
        args.iter().filter(|arg| *arg == "-i").count(),
        plan.inputs.len()
    );
    assert!(!args.iter().any(|arg| arg == "-c:s"));
}

#[test]
fn 软字幕追加一路输入与字幕编码() {
    let scratch = Scratch::new("subtitle-soft");
    let plan = plan_sequence(&with_subtitle(&scratch)).expect("能导出");
    let count = plan.inputs.len();
    let path = Path::new("临时字幕.ass");
    let args = plan.args_with_subtitle(path, SubtitleMode::Soft, "mov_text");
    assert_eq!(args.iter().filter(|arg| *arg == "-i").count(), count + 1);
    assert!(args.iter().any(|arg| arg == "临时字幕.ass"));
    assert!(args.iter().any(|arg| arg == &format!("{count}:s:0")));
    let codec = args.iter().position(|arg| arg == "-c:s").unwrap();
    assert_eq!(args[codec + 1], "mov_text");
    let filter_index = args
        .iter()
        .position(|arg| arg == "-filter_complex")
        .unwrap();
    assert!(!args[filter_index + 1].contains("subtitles="));
    assert!(args.iter().any(|arg| arg == "[outv]"), "画面不动");
}

#[test]
fn 模式与工作区都跟着导出设置走() {
    let scratch = Scratch::new("subtitle-settings");
    let project = with_subtitle(&scratch);
    let settings = ExportSettings {
        subtitle: SubtitleMode::Soft,
        ..ExportSettings::default()
    };
    let plan = plan_sequence_with(&project, Some((s(6), s(9))), &settings).expect("能导出");
    let attachment = plan.subtitle.as_ref().expect("带上字幕附件");
    assert_eq!(attachment.mode, SubtitleMode::Soft);
    // 字幕 5–8 秒与 6–9 秒的工作区相交后，从新的 0 秒起算、只留 2 秒。
    assert!(attachment.ass.contains("Dialogue: 0,0:00:00.00,0:00:02.00"));
    assert!(!attachment.ass.contains("0:00:05.00"));
    assert!(
        plan.filter
            .contains("[outv]trim=start=6.000000:end=9.000000")
    );
}
