//! 波纹修剪：头部 / 尾部收短之后，同轨后面的片段要跟着补上，不留空隙。

use super::*;

/// 一条轨上三段首尾相接：0–2、2–4、4–6。返回三段 id。
fn three(sequence: &mut Sequence) -> Vec<Id> {
    let track = sequence.tracks[0].id;
    vec![
        push(sequence, track, s(0), s(2), s(0)),
        push(sequence, track, s(0), s(2), s(2)),
        push(sequence, track, s(0), s(2), s(4)),
    ]
}

/// 三段应当首尾严丝合缝：既不重叠也不留空隙。
fn assert_abutting(track: &crate::timeline::Track) {
    for pair in track.clips.windows(2) {
        assert_eq!(
            pair[0].end(),
            pair[1].start,
            "片段之间不该有空隙或重叠：{:?} → {:?}",
            pair[0].end(),
            pair[1].start
        );
    }
}

#[test]
fn 波纹修剪尾把后面整段往前补() {
    let mut sequence = seq();
    let ids = three(&mut sequence);
    let next = applied(
        &sequence,
        EditOp::RippleTrimEnd {
            clip: ids[0],
            to: s(1),
        },
    );

    let track = &next.tracks[0];
    assert_eq!(track.clips[0].end(), s(1), "第一段收短一秒");
    assert_eq!(track.clips[0].source_out, s(1), "素材出点跟着走");
    assert_eq!(track.clips[1].start, s(1), "第二段顶上来，不留空隙");
    assert_eq!(track.clips[1].end(), s(3));
    assert_eq!(track.clips[2].start, s(3), "第三段也一起前移");
    assert_abutting(track);
}

#[test]
fn 波纹修剪头让片段原位变短并把后面拉过来() {
    let mut sequence = seq();
    let ids = three(&mut sequence);
    let next = applied(
        &sequence,
        EditOp::RippleTrimStart {
            clip: ids[1],
            to: s(3),
        },
    );

    let track = &next.tracks[0];
    let trimmed = &track.clips[1];
    assert_eq!(trimmed.start, s(2), "起点不动：波纹修剪不留空隙");
    assert_eq!(trimmed.source_in, s(1), "入点往后收了 1 秒");
    assert_eq!(trimmed.end(), s(3), "终点跟着前移");
    assert_eq!(track.clips[2].start, s(3), "后面那段补上来");
    assert_abutting(track);
}

#[test]
fn 波纹修剪往回放会把后面的片段推开() {
    let mut sequence = seq();
    let ids = three(&mut sequence);
    let next = applied(
        &sequence,
        EditOp::RippleTrimEnd {
            clip: ids[0],
            to: s(3),
        },
    );

    let track = &next.tracks[0];
    assert_eq!(track.clips[0].end(), s(3), "尾端拉长一秒");
    assert_eq!(track.clips[1].start, s(3), "第二段被推开，仍然首尾相接");
    assert_abutting(track);
}

#[test]
fn 波纹修剪收没了就拒绝() {
    let mut sequence = seq();
    let ids = three(&mut sequence);
    // 头部收到自己终点之外：整段会被收没。
    assert!(
        apply(
            &sequence,
            &EditOp::RippleTrimStart {
                clip: ids[0],
                to: s(2),
            }
        )
        .is_err()
    );
    // 尾部收到自己起点之前。
    assert!(
        apply(
            &sequence,
            &EditOp::RippleTrimEnd {
                clip: ids[0],
                to: s(-1),
            }
        )
        .is_err()
    );
    // 素材入点不够（往回放到负数帧）也拒绝。
    assert!(
        apply(
            &sequence,
            &EditOp::RippleTrimStart {
                clip: ids[0],
                to: s(-1),
            }
        )
        .is_err()
    );
    // 原序列不受影响：失败不该改到输入。
    assert_eq!(sequence.tracks[0].clips[0].start, s(0));
    assert_eq!(sequence.tracks[0].clips[0].end(), s(2));
}

#[test]
fn 波纹修剪只动同一条轨() {
    let mut sequence = seq();
    let ids = three(&mut sequence);
    let audio = sequence.add_track(TrackKind::Audio);
    let audio_clip = push(&mut sequence, audio, s(0), s(4), s(0));

    let next = applied(
        &sequence,
        EditOp::RippleTrimEnd {
            clip: ids[0],
            to: s(1),
        },
    );

    assert_eq!(
        next.track(audio).unwrap().clip(audio_clip).unwrap().start,
        s(0),
        "其它轨道不跟着动（多轨对齐仍要手工来）"
    );
    assert_eq!(next.track(audio).unwrap().clips[0].end(), s(4));
}
