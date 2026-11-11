//! 关键帧标记的拖动：时间线上的一根标记是所有属性共用的，挪的时候必须整组走。
//!
//! 时间存的是**片段内相对时间**，所以这里也验证「拖到片段两端之外夹回去」与
//! 「原地松手不算一次编辑」这两件界面依赖的语义。

use super::*;
use crate::timeline::Property;

#[test]
fn 拖动标记把整组点挪到新时刻() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let clip = push(&mut sequence, track, Tick::ZERO, s(4), s(0));
    {
        let item = sequence.clip_mut(clip).unwrap();
        item.keys.ensure(Property::Scale).insert(s(1), 2.0);
        item.keys.ensure(Property::Opacity).insert(s(1), 0.0);
        item.keys.ensure(Property::Opacity).insert(s(2), 1.0);
    }

    let next = applied(
        &sequence,
        EditOp::MoveKeyframe {
            clip,
            from: s(1),
            to: s(3),
        },
    );
    let keys = &next.clip(clip).unwrap().keys;
    assert_eq!(keys.marks(), vec![s(2), s(3)], "1 秒的整组点挪到了 3 秒");
    assert_eq!(keys.value(Property::Scale, s(3)), 2.0, "缩放跟着走");
    assert_eq!(keys.value(Property::Opacity, s(3)), 0.0, "不透明度也跟着走");
    assert_eq!(
        keys.value(Property::Scale, s(1)),
        2.0,
        "只剩一个点时整段都取这个值（端点外钳制，不外推）"
    );
}

#[test]
fn 拖到片段两端之外被夹回去() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let clip = push(&mut sequence, track, Tick::ZERO, s(4), s(0));
    sequence
        .clip_mut(clip)
        .unwrap()
        .keys
        .ensure(Property::Scale)
        .insert(s(1), 2.0);

    let after = applied(
        &sequence,
        EditOp::MoveKeyframe {
            clip,
            from: s(1),
            to: s(30),
        },
    );
    assert_eq!(
        after.clip(clip).unwrap().keys.marks(),
        vec![s(4)],
        "夹到片段末尾"
    );

    let before = applied(
        &sequence,
        EditOp::MoveKeyframe {
            clip,
            from: s(1),
            to: s(-5),
        },
    );
    assert_eq!(
        before.clip(clip).unwrap().keys.marks(),
        vec![Tick::ZERO],
        "夹到片段开头"
    );
}

#[test]
fn 目标时刻已有点时后写的覆盖() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let clip = push(&mut sequence, track, Tick::ZERO, s(4), s(0));
    {
        let item = sequence.clip_mut(clip).unwrap();
        item.keys.ensure(Property::Scale).insert(s(1), 2.0);
        item.keys.ensure(Property::Scale).insert(s(3), 3.0);
    }

    let next = applied(
        &sequence,
        EditOp::MoveKeyframe {
            clip,
            from: s(1),
            to: s(3),
        },
    );
    let curve = next
        .clip(clip)
        .unwrap()
        .keys
        .curve(Property::Scale)
        .unwrap()
        .clone();
    assert_eq!(curve.keys.len(), 1, "同一个时刻只留一个点");
    assert_eq!(curve.keys[0].at, s(3));
    assert_eq!(curve.keys[0].value, 2.0, "挪过来的值覆盖原值");
}

#[test]
fn 没抓住点或原地松手时序列原样() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let clip = push(&mut sequence, track, Tick::ZERO, s(4), s(0));
    sequence
        .clip_mut(clip)
        .unwrap()
        .keys
        .ensure(Property::Scale)
        .insert(s(1), 2.0);

    for (from, to) in [(s(2), s(3)), (s(1), s(1))] {
        let next = applied(&sequence, EditOp::MoveKeyframe { clip, from, to });
        assert_eq!(next, sequence, "没有可挪的点时序列逐字段相等");
    }
}

#[test]
fn 锁轨与未知片段拒绝() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let clip = push(&mut sequence, track, Tick::ZERO, s(4), s(0));
    sequence
        .clip_mut(clip)
        .unwrap()
        .keys
        .ensure(Property::Scale)
        .insert(s(1), 2.0);
    sequence.track_mut(track).unwrap().locked = true;

    assert_eq!(
        apply(
            &sequence,
            &EditOp::MoveKeyframe {
                clip,
                from: s(1),
                to: s(2),
            },
        ),
        Err(EditError::TrackLocked(track))
    );

    sequence.track_mut(track).unwrap().locked = false;
    assert_eq!(
        apply(
            &sequence,
            &EditOp::MoveKeyframe {
                clip: Id::new(999),
                from: s(1),
                to: s(2),
            },
        ),
        Err(EditError::UnknownClip(Id::new(999)))
    );
}
