//! 删除：删片段、删轨道要顺手把挂在它们身上的东西一起带走。
//!
//! 与「删除留下空隙 / 波纹删除」不是一件事：那是放置语义（怎么填补），
//! 这里是数据完整性（转场、片段、轨道不许留脏引用）。

use super::*;

#[test]
fn 删除片段时连带清掉挂在它身上的转场() {
    use crate::timeline::{Transition, TransitionKind};

    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let first = push(&mut sequence, track, s(0), s(2), s(0));
    let second = push(&mut sequence, track, s(0), s(2), s(2));
    let transition = sequence.alloc_id();
    sequence
        .track_mut(track)
        .unwrap()
        .transitions
        .push(Transition::new(
            transition,
            first,
            second,
            TransitionKind::CrossDissolve,
            Tick::from_millis(500),
        ));
    assert!(sequence.validate().is_ok());

    let next = applied(
        &sequence,
        EditOp::Remove {
            clips: vec![second],
        },
    );
    assert!(next.tracks[0].transitions.is_empty(), "转场不该留下脏数据");
}

#[test]
fn 删除不存在的片段返回错误() {
    let sequence = seq();
    assert_eq!(
        apply(
            &sequence,
            &EditOp::Remove {
                clips: vec![Id::new(42)]
            }
        ),
        Err(EditError::UnknownClip(Id::new(42)))
    );
}

#[test]
fn 删除轨道时连片段与转场一起走() {
    let mut sequence = seq();
    let video = sequence.tracks[0].id;
    let audio = sequence.tracks[1].id;
    push(&mut sequence, video, s(0), s(2), s(0));
    push(&mut sequence, audio, s(0), s(3), s(0));
    sequence.add_track(TrackKind::Video);

    let next = applied(&sequence, EditOp::RemoveTrack { track: video });
    assert_eq!(next.tracks.len(), 2);
    assert_eq!(next.tracks[0].id, audio, "音频轨留在原处");
    assert_eq!(next.tracks[1].kind, TrackKind::Video);
    assert!(next.validate().is_ok());
    assert_eq!(next.duration(), s(3));
}

#[test]
fn 锁定或不存在的轨道不能删() {
    let mut sequence = seq();
    let video = sequence.tracks[0].id;
    sequence.track_mut(video).unwrap().locked = true;
    assert_eq!(
        apply(&sequence, &EditOp::RemoveTrack { track: video }),
        Err(EditError::TrackLocked(video))
    );
    assert_eq!(
        apply(
            &sequence,
            &EditOp::RemoveTrack {
                track: Id::new(999)
            }
        ),
        Err(EditError::UnknownTrack(Id::new(999)))
    );
}
