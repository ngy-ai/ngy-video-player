//! 放置语义：覆盖、插入、移动落位与波纹删除（M9-T04）。
//!
//! 这些用例的共用点是「落点压到已有内容」，所以单开一个文件：
//! 前一半测试的是单点编辑（修剪、滑动），两者的失败方式完全不同。

use super::*;

#[test]
fn 覆盖压在片段中间会切掉中段() {
    let mut sequence = seq();
    let video = sequence.tracks[0].id;
    let original = push(&mut sequence, video, Tick::ZERO, s(4), Tick::ZERO);

    let next = applied(
        &sequence,
        EditOp::Overwrite {
            track: video,
            at: s(1),
            clip: ClipSpec::new(AssetId::new(1), Tick::ZERO, s(1)),
        },
    );

    // 左段被收边、右段切出来、新片段夹在中间。
    assert_eq!(starts(&next, 0), vec![s(0), s(1), s(2)]);
    assert_eq!(next.tracks[0].clips[0].id, original, "左段保留原 id");
    assert_eq!(next.tracks[0].clips[0].end(), s(1), "左段出点收到落点");
    assert_eq!(next.tracks[0].clips[1].source_out, s(1), "新片段长度 1 秒");
    assert_eq!(next.tracks[0].clips[2].end(), s(4), "右段还有 2 秒");
    assert_eq!(next.duration(), s(4), "覆盖不改变整条序列的长度");
}

#[test]
fn 覆盖整段时不留空壳() {
    let mut sequence = seq();
    let video = sequence.tracks[0].id;
    push(&mut sequence, video, Tick::ZERO, s(2), Tick::ZERO);

    let next = applied(
        &sequence,
        EditOp::Overwrite {
            track: video,
            at: s(0),
            clip: ClipSpec::new(AssetId::new(1), Tick::ZERO, s(2)),
        },
    );
    assert_eq!(next.tracks[0].clips.len(), 1, "被完全盖住的片段应当消失");
    assert_eq!(next.duration(), s(2));
}

#[test]
fn 插入把后续片段整体右移() {
    let mut sequence = seq();
    let video = sequence.tracks[0].id;
    push(&mut sequence, video, Tick::ZERO, s(2), Tick::ZERO);
    push(&mut sequence, video, Tick::ZERO, s(2), s(2));

    let next = applied(
        &sequence,
        EditOp::InsertAt {
            track: video,
            at: s(1),
            clip: ClipSpec::new(AssetId::new(1), Tick::ZERO, s(1)),
        },
    );

    // 第一段在落点处切开（右半段被推开），第二段整段被推开 1 秒。
    assert_eq!(starts(&next, 0), vec![s(0), s(1), s(2), s(3)]);
    assert_eq!(next.tracks[0].clips[2].start, s(2), "切出来的右段从 2 秒起");
    assert_eq!(next.tracks[0].clips[3].start, s(3), "后面的片段右移 1 秒");
    assert_eq!(next.duration(), s(5), "插入让序列变长");
}

#[test]
fn 插入在序列最前面也成立() {
    let mut sequence = seq();
    let video = sequence.tracks[0].id;
    push(&mut sequence, video, Tick::ZERO, s(2), Tick::ZERO);

    let next = applied(
        &sequence,
        EditOp::InsertAt {
            track: video,
            at: Tick::ZERO,
            clip: ClipSpec::new(AssetId::new(1), Tick::ZERO, s(1)),
        },
    );
    assert_eq!(starts(&next, 0), vec![s(0), s(1)]);
    assert_eq!(next.duration(), s(3));
}

#[test]
fn 移动用覆盖语义落位() {
    let mut sequence = seq();
    let video = sequence.tracks[0].id;
    push(&mut sequence, video, Tick::ZERO, s(4), Tick::ZERO);
    let moving = push(&mut sequence, video, Tick::ZERO, s(2), s(6));

    let next = applied(
        &sequence,
        EditOp::MoveOverwrite {
            clip: moving,
            to_track: video,
            start: s(1),
        },
    );

    let clips = &next.tracks[0].clips;
    assert_eq!(clips.len(), 3);
    assert_eq!(clips[0].end(), s(1), "被压住的片段收边");
    assert_eq!(clips[1].id, moving, "移动的片段落在 1 秒处");
    assert_eq!(clips[1].start, s(1));
    assert_eq!(clips[2].start, s(3), "右段从覆盖区后面接上");
}

#[test]
fn 移动用插入语义把后续推开() {
    let mut sequence = seq();
    let video = sequence.tracks[0].id;
    push(&mut sequence, video, Tick::ZERO, s(4), Tick::ZERO);
    let moving = push(&mut sequence, video, Tick::ZERO, s(1), s(6));

    let next = applied(
        &sequence,
        EditOp::MoveInsert {
            clip: moving,
            to_track: video,
            start: s(2),
        },
    );

    let clips = &next.tracks[0].clips;
    assert_eq!(clips.len(), 3);
    assert_eq!(clips[0].end(), s(2), "左段收在落点");
    assert_eq!(clips[1].id, moving);
    assert_eq!(clips[1].start, s(2));
    assert_eq!(clips[2].start, s(3), "右段被推开 1 秒");
    assert_eq!(next.duration(), s(5));
}

#[test]
fn 波纹删除把后面接上() {
    let mut sequence = seq();
    let video = sequence.tracks[0].id;
    push(&mut sequence, video, Tick::ZERO, s(2), Tick::ZERO);
    let middle = push(&mut sequence, video, Tick::ZERO, s(2), s(2));
    push(&mut sequence, video, Tick::ZERO, s(2), s(4));

    let next = applied(
        &sequence,
        EditOp::RippleDelete {
            clips: vec![middle],
        },
    );

    assert_eq!(starts(&next, 0), vec![s(0), s(2)], "接缝不留空隙");
    assert_eq!(next.duration(), s(4), "整条序列短了 2 秒");
}

#[test]
fn 波纹删除不会把时间推成负数() {
    let mut sequence = seq();
    let video = sequence.tracks[0].id;
    let first = push(&mut sequence, video, Tick::ZERO, s(2), Tick::ZERO);
    push(&mut sequence, video, Tick::ZERO, s(1), s(5));

    let next = applied(&sequence, EditOp::RippleDelete { clips: vec![first] });
    assert_eq!(starts(&next, 0), vec![s(3)], "后面的片段只前移被删的长度");
}

#[test]
fn 锁定的轨道拒绝插入与覆盖() {
    let mut sequence = seq();
    let video = sequence.tracks[0].id;
    push(&mut sequence, video, Tick::ZERO, s(2), Tick::ZERO);
    sequence.tracks[0].locked = true;

    for op in [
        EditOp::InsertAt {
            track: video,
            at: s(0),
            clip: ClipSpec::new(AssetId::new(1), Tick::ZERO, s(1)),
        },
        EditOp::Overwrite {
            track: video,
            at: s(0),
            clip: ClipSpec::new(AssetId::new(1), Tick::ZERO, s(1)),
        },
    ] {
        assert_eq!(apply(&sequence, &op), Err(EditError::TrackLocked(video)));
    }
}
