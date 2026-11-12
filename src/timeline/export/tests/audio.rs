//! 音频混音的形状：多轨 `amix`、轨道音量 `volume=`、独奏挤掉别的轨。

use super::*;

#[test]
fn 两条音频轨按amix混合() {
    let scratch = Scratch::new("amix");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    let asset = project.assets[0].id;

    let first = project.sequence_mut().tracks[1].id;
    let id = project.sequence_mut().alloc_id();
    project
        .sequence_mut()
        .track_mut(first)
        .expect("音频轨存在")
        .clips
        .push(ClipItem::new(id, asset, s(0), s(2), Tick::ZERO));
    add_track_clip(
        &mut project,
        TrackKind::Audio,
        asset,
        s(0),
        s(2),
        Tick::ZERO,
    );

    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    assert!(plan.filter.contains("[layera0]"), "{}", plan.filter);
    assert!(plan.filter.contains("[layera1]"), "{}", plan.filter);
    assert!(
        plan.filter.contains("amix=inputs=2:duration=longest"),
        "{}",
        plan.filter
    );
    assert_eq!(plan.audio_label.as_deref(), Some("[outa]"));
}

/// 轨道音量落成 `volume` 滤镜：原样（0 dB）不插多余的一道，改过的写进每条片段链。
#[test]
fn 轨道音量写成volume滤镜() {
    let scratch = Scratch::new("gain");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    let asset = project.assets[0].id;
    let audio = project.sequence_mut().tracks[1].id;
    let id = project.sequence_mut().alloc_id();
    project
        .sequence_mut()
        .track_mut(audio)
        .expect("音频轨存在")
        .clips
        .push(ClipItem::new(id, asset, s(0), s(2), Tick::ZERO));

    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    assert!(
        !plan.filter.contains("volume="),
        "没改过音量就不插滤镜：{}",
        plan.filter
    );

    project.sequence_mut().track_mut(audio).unwrap().gain = 0.5;
    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    assert!(
        plan.filter.contains("volume=0.5000"),
        "轨道音量要进滤镜链：{}",
        plan.filter
    );
}

/// 独奏把别的音频轨挤出混音：只剩一路时连 `amix` 都不插。
#[test]
fn 独奏时只有独奏的音频轨进混音() {
    let scratch = Scratch::new("solo");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    let asset = project.assets[0].id;
    let first = project.sequence_mut().tracks[1].id;
    let first_clip = project.sequence_mut().alloc_id();
    project
        .sequence_mut()
        .track_mut(first)
        .expect("音频轨存在")
        .clips
        .push(ClipItem::new(first_clip, asset, s(0), s(2), Tick::ZERO));
    let second_clip = add_track_clip(
        &mut project,
        TrackKind::Audio,
        asset,
        s(0),
        s(2),
        Tick::ZERO,
    );
    let second = project
        .sequence()
        .clip_track(second_clip)
        .expect("片段在第二条音频轨上");

    let plan = plan_sequence(&project).expect("能导出");
    assert!(
        plan.filter.contains("amix=inputs=2"),
        "两条轨都参与混音：{}",
        plan.filter
    );

    project.sequence_mut().track_mut(second).unwrap().solo = true;
    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    assert!(
        !plan.filter.contains("amix="),
        "只剩一路就不用混音：{}",
        plan.filter
    );
    assert!(
        plan.filter.contains(&format!("[a{}]", second_clip.raw())),
        "独奏那条还在：{}",
        plan.filter
    );
    assert!(
        !plan.filter.contains(&format!("[a{}]", first_clip.raw())),
        "被独奏挤掉的轨不该出现：{}",
        plan.filter
    );
}
