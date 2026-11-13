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
fn 速度曲线让后面的片段按积分让位() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let first = push(&mut sequence, track, s(0), s(4), s(0));
    push(&mut sequence, track, s(0), s(2), s(4));

    // 全程 2x：时长砍半，后面的片段跟着前移到 2 秒。
    let mut curve = crate::timeline::SpeedCurve::default();
    curve.insert(0.0, 2.0);
    let next = applied(
        &sequence,
        EditOp::SetRemap {
            clip: first,
            curve: curve.clone(),
        },
    );
    assert_eq!(next.clip(first).unwrap().motion(), s(2));
    assert_eq!(starts(&next, 0), [s(0), s(2)]);

    // 清空曲线：长度回到素材跨度，后面的片段跟着退回原位。
    let next = applied(
        &next,
        EditOp::SetRemap {
            clip: first,
            curve: crate::timeline::SpeedCurve::default(),
        },
    );
    assert!(!next.clip(first).unwrap().has_remap());
    assert_eq!(starts(&next, 0), [s(0), s(4)]);
}

#[test]
fn 曲线没变时不落撤销且坏值被拒() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let id = push(&mut sequence, track, s(0), s(4), s(0));

    let mut curve = crate::timeline::SpeedCurve::default();
    curve.insert(0.0, 2.0);
    let next = applied(
        &sequence,
        EditOp::SetRemap {
            clip: id,
            curve: curve.clone(),
        },
    );
    assert_eq!(
        apply(&next, &EditOp::SetRemap { clip: id, curve }),
        Ok(next.clone()),
        "同一个曲线再来一次不算改动"
    );

    let bad = crate::timeline::SpeedCurve {
        points: vec![crate::timeline::SpeedPoint {
            at: 0.0,
            rate: f32::NAN,
        }],
    };
    assert_eq!(
        apply(
            &sequence,
            &EditOp::SetRemap {
                clip: id,
                curve: bad
            }
        ),
        Err(EditError::BadRate)
    );

    sequence.track_mut(track).unwrap().locked = true;
    assert_eq!(
        apply(
            &sequence,
            &EditOp::SetRemap {
                clip: id,
                curve: crate::timeline::SpeedCurve::default()
            }
        ),
        Err(EditError::TrackLocked(track))
    );
}

/// 一条 4 秒素材、从 2× 线性降到 0.5× 的曲线，时间线上约 3.696 秒。
fn curved_at(sequence: &mut Sequence, track: Id, start: Tick) -> Id {
    let id = push(sequence, track, s(0), s(4), start);
    let mut curve = crate::timeline::SpeedCurve::default();
    curve.insert(0.0, 2.0);
    curve.insert(4.0, 0.5);
    crate::timeline::set_remap(sequence, id, &curve);
    id
}

/// 从 0 秒开始的曲线片段。
fn curved(sequence: &mut Sequence, track: Id) -> Id {
    curved_at(sequence, track, s(0))
}

#[test]
fn 切开曲线片段时两段各留一半曲线() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let id = curved(&mut sequence, track);
    let whole = sequence.clip(id).unwrap().duration();

    let next = applied(
        &sequence,
        EditOp::Split {
            clip: id,
            at: Tick::from_millis(1500),
        },
    );
    let clips = &next.tracks[0].clips;
    assert_eq!(clips.len(), 2);
    assert!(
        clips[0].has_remap() && clips[1].has_remap(),
        "两段都留着曲线"
    );
    assert_eq!(clips[0].end(), clips[1].start, "首尾相接");
    assert_eq!(clips[0].source_out, clips[1].source_in, "素材不丢不重");
    // 左段正好占了时间线的前 1.5 秒，两段加起来还是原来的时长。
    assert!(
        (clips[0].duration().secs() - 1.5).abs() < 0.05,
        "左段 {} 秒",
        clips[0].duration().secs()
    );
    let total = clips[0].duration() + clips[1].duration();
    assert!(
        (total.secs() - whole.secs()).abs() < 0.05,
        "两段合计 {} 秒，整段 {} 秒",
        total.secs(),
        whole.secs()
    );
    next.validate().expect("切完仍然合法");
}

#[test]
fn 曲线片段只能往里修不能往外拉() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let id = curved(&mut sequence, track);
    let whole = sequence.clip(id).unwrap().duration();

    // 往里收：尾收到时间线 1 秒，时长跟着变成 1 秒，曲线切掉后半段。
    let next = applied(&sequence, EditOp::TrimEnd { clip: id, to: s(1) });
    let trimmed = next.clip(id).unwrap();
    assert!(trimmed.has_remap());
    assert!(
        (trimmed.duration().secs() - 1.0).abs() < 0.05,
        "收完 {} 秒",
        trimmed.duration().secs()
    );

    // 往里收头：起点挪到 1 秒，剩下的是曲线后半段的积分。
    let next = applied(&sequence, EditOp::TrimStart { clip: id, to: s(1) });
    let head = next.clip(id).unwrap();
    assert!(head.has_remap());
    assert!(
        (head.duration().secs() - (whole.secs() - 1.0)).abs() < 0.05,
        "收头之后 {} 秒",
        head.duration().secs()
    );
    assert_eq!(head.start, s(1));

    // 往外拉要借曲线没覆盖的素材：明确拒绝，不要算出一条时长错乱的片段。
    assert_eq!(
        apply(
            &sequence,
            &EditOp::TrimEnd {
                clip: id,
                to: whole + s(2)
            }
        ),
        Err(EditError::CurveShift)
    );
    // 收头只能往里：片段挪到 2 秒起，再往 1 秒拉就是往外借素材。
    let mut later = seq();
    let track = later.tracks[0].id;
    let id = curved_at(&mut later, track, s(2));
    assert_eq!(
        apply(&later, &EditOp::TrimStart { clip: id, to: s(1) }),
        Err(EditError::CurveShift)
    );
}

#[test]
fn 覆盖曲线片段时按曲线切开() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let id = curved(&mut sequence, track);
    let whole = sequence.clip(id).unwrap().duration();

    // 在 1 秒处覆盖 1 秒：左段收到 1 秒、右段从 2 秒接回去，两段都该留着曲线。
    let next = applied(
        &sequence,
        EditOp::Overwrite {
            track,
            at: s(1),
            clip: ClipSpec::new(AssetId::new(1), Tick::ZERO, s(1)),
        },
    );
    let pieces: Vec<_> = next.tracks[0]
        .clips
        .iter()
        .filter(|clip| clip.has_remap())
        .collect();
    assert_eq!(pieces.len(), 2, "左右两段都还带着曲线");
    assert_eq!(pieces[1].start, s(2), "右半段从覆盖区之后接上");
    // 左右两段加上被覆盖的 1 秒，总长和原来对得上（曲线积分允许几十微秒的差）。
    let total = next.duration();
    assert!(
        (total.secs() - whole.secs()).abs() < 0.05,
        "覆盖完总长 {} 秒，原来 {} 秒",
        total.secs(),
        whole.secs()
    );
    next.validate().expect("覆盖完仍然合法");
}

#[test]
fn 曲线片段不能滑移滑动卷动() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let id = curved(&mut sequence, track);
    assert_eq!(
        apply(&sequence, &EditOp::Slip { clip: id, by: s(1) }),
        Err(EditError::CurveShift)
    );

    // 卷动 / 滑动要有紧挨着的邻居才谈得上：前后各接一段。
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    push(&mut sequence, track, s(0), s(2), s(0));
    let id = curved_at(&mut sequence, track, s(2));
    let end = sequence.clip(id).unwrap().end();
    push(&mut sequence, track, s(0), s(2), end);
    assert_eq!(
        apply(&sequence, &EditOp::Roll { clip: id, by: s(1) }),
        Err(EditError::CurveShift)
    );
    assert_eq!(
        apply(&sequence, &EditOp::Slide { clip: id, by: s(1) }),
        Err(EditError::CurveShift)
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
