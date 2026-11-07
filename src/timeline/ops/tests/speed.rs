//! 变速与定格（M9-T09）：走 `EditOp` 之后，锁轨与撤销才和别的编辑一样生效。
//!
//! 波纹让位本身在 `timeline/speed.rs` 里，这里的用例钉的是「操作层」：
//! 前置校验、未知片段、以及值没变时不产生新序列。

use super::*;

#[test]
fn 变速让同轨后面的片段跟着前移() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let first = push(&mut sequence, track, s(0), s(2), s(0));
    push(&mut sequence, track, s(0), s(2), s(2));

    let next = applied(
        &sequence,
        EditOp::SetSpeed {
            clip: first,
            speed: 2.0,
        },
    );
    assert_eq!(next.clip(first).unwrap().duration(), s(1));
    assert_eq!(starts(&next, 0), [s(0), s(1)], "后一段跟着前移");
}

#[test]
fn 定格把后面的片段往后推() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let first = push(&mut sequence, track, s(0), s(2), s(0));
    push(&mut sequence, track, s(0), s(2), s(2));

    let next = applied(
        &sequence,
        EditOp::SetHold {
            clip: first,
            hold: s(3),
        },
    );
    assert_eq!(next.clip(first).unwrap().duration(), s(5));
    assert_eq!(starts(&next, 0), [s(0), s(5)]);

    // 取消定格：长度回到素材跨度，后面的片段跟着前移。
    let next = applied(
        &next,
        EditOp::SetHold {
            clip: first,
            hold: Tick::ZERO,
        },
    );
    assert_eq!(next.clip(first).unwrap().duration(), s(2));
    assert_eq!(starts(&next, 0), [s(0), s(2)]);
}

#[test]
fn 值没变时序列原样返回() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let id = push(&mut sequence, track, s(0), s(2), s(0));

    let same = apply(
        &sequence,
        &EditOp::SetSpeed {
            clip: id,
            speed: 1.0,
        },
    )
    .unwrap();
    assert_eq!(same, sequence, "原速不算改动，不该多一步撤销");
    let same = apply(
        &sequence,
        &EditOp::SetHold {
            clip: id,
            hold: Tick::ZERO,
        },
    )
    .unwrap();
    assert_eq!(same, sequence);
}

#[test]
fn 倍率夹到量程而坏值被拒绝() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let id = push(&mut sequence, track, s(0), s(4), s(0));

    let next = applied(
        &sequence,
        EditOp::SetSpeed {
            clip: id,
            speed: 100.0,
        },
    );
    assert_eq!(next.clip(id).unwrap().speed, ClipItem::MAX_SPEED);
    assert_eq!(next.clip(id).unwrap().duration(), s(1), "4 秒素材 4 倍速");

    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(
            apply(
                &sequence,
                &EditOp::SetSpeed {
                    clip: id,
                    speed: bad
                }
            ),
            Err(EditError::BadRate)
        );
    }
    assert_eq!(
        apply(
            &sequence,
            &EditOp::SetHold {
                clip: id,
                hold: s(-1)
            }
        ),
        Err(EditError::BadRate),
        "负数定格要显式给 0 才是取消"
    );
}

#[test]
fn 锁定轨道与未知片段都拒绝() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let id = push(&mut sequence, track, s(0), s(2), s(0));
    sequence.track_mut(track).unwrap().locked = true;

    assert_eq!(
        apply(
            &sequence,
            &EditOp::SetSpeed {
                clip: id,
                speed: 2.0
            }
        ),
        Err(EditError::TrackLocked(track))
    );
    assert_eq!(
        apply(
            &sequence,
            &EditOp::SetHold {
                clip: id,
                hold: s(1)
            }
        ),
        Err(EditError::TrackLocked(track))
    );

    sequence.track_mut(track).unwrap().locked = false;
    assert_eq!(
        apply(
            &sequence,
            &EditOp::SetSpeed {
                clip: Id::new(999),
                speed: 2.0
            }
        ),
        Err(EditError::UnknownClip(Id::new(999)))
    );
}
