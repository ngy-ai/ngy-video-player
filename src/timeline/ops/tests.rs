//! 编辑操作的边界：重叠、修剪到 0、切点在边缘、缺少邻居。

use super::*;
use crate::timeline::{Rational, TrackKind};

fn seq() -> Sequence {
    Sequence::new("测试", Rational::FPS_25, 1920, 1080)
}

/// 第 `n` 秒，测试里用整数秒比 0.001 漂移好读。
fn s(n: i64) -> Tick {
    Tick::from_micros(n * 1_000_000)
}

fn push(sequence: &mut Sequence, track: Id, source_in: Tick, source_out: Tick, start: Tick) -> Id {
    let id = sequence.alloc_id();
    let track = sequence.track_mut(track).unwrap();
    track.clips.push(ClipItem::new(
        id,
        AssetId::new(1),
        source_in,
        source_out,
        start,
    ));
    track.sort_clips();
    id
}

fn insert(sequence: &Sequence, track: Id, at: Tick, duration: Tick) -> Result<Sequence, EditError> {
    apply(
        sequence,
        &EditOp::Insert {
            track,
            at,
            clip: ClipSpec::new(AssetId::new(1), Tick::ZERO, duration),
        },
    )
}

/// 每条用例收尾都跑一次模型校验：操作成功却留下重叠，等于没做对。
fn applied(sequence: &Sequence, op: EditOp) -> Sequence {
    let next = apply(sequence, &op).expect("操作应当成功");
    next.validate().expect("操作后模型应当合法");
    next
}

fn starts(sequence: &Sequence, track: usize) -> Vec<Tick> {
    sequence.tracks[track]
        .clips
        .iter()
        .map(|clip| clip.start)
        .collect()
}

#[test]
fn 删除空列表返回空结果() {
    let sequence = seq();
    assert_eq!(
        remove_clips(&sequence, &[]).unwrap(),
        None,
        "没选中不算错误"
    );
}

#[test]
fn 一次删除多个片段() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let first = push(&mut sequence, track, Tick::ZERO, s(2), s(0));
    let second = push(&mut sequence, track, s(3), s(2), s(0));
    let keep = push(&mut sequence, track, s(6), s(2), s(0));

    let next = remove_clips(&sequence, &[first, second])
        .unwrap()
        .expect("有得删");
    assert_eq!(next.tracks[0].clips.len(), 1);
    assert_eq!(next.tracks[0].clips[0].id, keep);

    // 再删一次同一批 id：片段已经不在，要报错而不是静默成功。
    assert_eq!(
        remove_clips(&next, &[first]).unwrap_err(),
        EditError::UnknownClip(first)
    );
}

#[test]
fn 在播放头处切开所有跨过它的片段() {
    let mut sequence = seq();
    let video = sequence.tracks[0].id;
    let audio = sequence.tracks[1].id;
    push(&mut sequence, video, Tick::ZERO, s(4), s(0));
    push(&mut sequence, audio, Tick::ZERO, s(4), s(0));

    let (next, cuts) = split_at(&sequence, s(2))
        .unwrap()
        .expect("两条轨道各切一刀");
    assert_eq!(cuts, 2);
    assert_eq!(next.tracks[0].clips.len(), 2);
    assert_eq!(next.tracks[1].clips.len(), 2);

    // 左半段出点就是右半段入点，时间严丝合缝。
    let left = &next.tracks[0].clips[0];
    let right = &next.tracks[0].clips[1];
    assert_eq!(left.end(), right.start);
    assert_eq!(left.source_out, right.source_in);
    next.validate().expect("切完仍然合法");

    assert_eq!(
        split_at(&next, s(2)).unwrap(),
        None,
        "同一点再切一次已经没有内容"
    );
}

#[test]
fn 切割跳过锁定轨道与片段边界() {
    let mut sequence = seq();
    let video = sequence.tracks[0].id;
    let audio = sequence.tracks[1].id;
    push(&mut sequence, video, Tick::ZERO, s(4), s(0));
    push(&mut sequence, audio, Tick::ZERO, s(4), s(0));
    sequence.track_mut(video).unwrap().locked = true;

    let (next, cuts) = split_at(&sequence, s(2)).unwrap().expect("音频轨还能切");
    assert_eq!(cuts, 1);
    assert_eq!(next.tracks[0].clips.len(), 1, "锁定轨道不动");
    assert_eq!(next.tracks[1].clips.len(), 2);

    // 正压在边界上不算「跨过」：切下去只会得到一个 0 长的片段。
    assert_eq!(split_at(&sequence, s(4)).unwrap(), None);
    assert_eq!(split_at(&sequence, Tick::ZERO).unwrap(), None);
}

#[test]
fn 锁定轨道拒绝编辑且原序列不动() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let clip = push(&mut sequence, track, Tick::ZERO, s(4), s(0));
    sequence.track_mut(track).unwrap().locked = true;

    assert_eq!(
        insert(&sequence, track, s(5), s(1)).unwrap_err(),
        EditError::TrackLocked(track),
        "锁定轨道上不能插入"
    );
    assert_eq!(
        apply(&sequence, &EditOp::TrimEnd { clip, to: s(3) }).unwrap_err(),
        EditError::TrackLocked(track),
        "锁定轨道上不能修剪"
    );
    assert_eq!(
        apply(&sequence, &EditOp::Remove { clips: vec![clip] }).unwrap_err(),
        EditError::TrackLocked(track),
        "锁定轨道上不能删除"
    );
    assert_eq!(sequence.tracks[0].clips.len(), 1, "失败的操作不留痕迹");
}

#[test]
fn 目标轨道锁定时拒绝换轨() {
    let mut sequence = seq();
    let video = sequence.tracks[0].id;
    // 必须和源轨道同类型：类型不符会先被 `WrongTrackKind` 拦下，测不到锁定。
    let second_video = sequence.add_track(TrackKind::Video);
    let clip = push(&mut sequence, video, Tick::ZERO, s(2), s(0));
    sequence.track_mut(second_video).unwrap().locked = true;

    let error = apply(
        &sequence,
        &EditOp::Move {
            clip,
            to_track: second_video,
            start: s(0),
        },
    )
    .unwrap_err();
    assert_eq!(error, EditError::TrackLocked(second_video));
}

#[test]
fn 插入按起点排序() {
    let sequence = seq();
    let track = sequence.tracks[0].id;
    let sequence = insert(&sequence, track, s(5), s(2)).unwrap();
    let sequence = insert(&sequence, track, s(1), s(1)).unwrap();

    assert_eq!(starts(&sequence, 0), [s(1), s(5)]);
    assert_eq!(sequence.duration(), s(7));
}

#[test]
fn 插入重叠与非法时长被拒绝() {
    let sequence = seq();
    let track = sequence.tracks[0].id;
    let sequence = insert(&sequence, track, s(0), s(2)).unwrap();
    let before = sequence.clone();

    assert_eq!(
        insert(&sequence, track, s(1), s(2)),
        Err(EditError::Overlap(track))
    );
    assert_eq!(
        insert(&sequence, track, s(0), Tick::ZERO),
        Err(EditError::EmptyClip)
    );
    assert_eq!(
        insert(&sequence, track, Tick::from_micros(-1), s(1)),
        Err(EditError::NegativeTime)
    );
    assert_eq!(
        insert(&sequence, Id::new(999), s(10), s(1)),
        Err(EditError::UnknownTrack(Id::new(999)))
    );
    assert_eq!(sequence, before, "失败的操作不改原序列");

    // 首尾正好相接可以插。
    let sequence = insert(&sequence, track, s(2), s(1)).unwrap();
    assert_eq!(starts(&sequence, 0), [s(0), s(2)]);
}

#[test]
fn 移动片段可以换轨但不能压住别人() {
    let mut sequence = seq();
    let video = sequence.tracks[0].id;
    let audio = sequence.tracks[1].id;
    let second_video = sequence.add_track(TrackKind::Video);
    let moving = push(&mut sequence, video, s(0), s(2), s(0));
    push(&mut sequence, video, s(0), s(2), s(4));

    assert_eq!(
        apply(
            &sequence,
            &EditOp::Move {
                clip: moving,
                to_track: audio,
                start: s(10)
            }
        ),
        Err(EditError::WrongTrackKind),
        "视频片段不能扔进音频轨"
    );
    assert_eq!(
        apply(
            &sequence,
            &EditOp::Move {
                clip: moving,
                to_track: video,
                start: s(3)
            }
        ),
        Err(EditError::Overlap(video)),
        "落点和第三个片段压住了"
    );

    // 换到另一条视频轨。
    let sequence = applied(
        &sequence,
        EditOp::Move {
            clip: moving,
            to_track: second_video,
            start: s(10),
        },
    );
    assert_eq!(starts(&sequence, 0), [s(4)], "原轨道上只剩一个");
    assert_eq!(
        sequence
            .track(second_video)
            .unwrap()
            .clips
            .iter()
            .map(|clip| clip.start)
            .collect::<Vec<_>>(),
        [s(10)]
    );
    assert_eq!(sequence.clip(moving).unwrap().end(), s(12));
    assert_eq!(sequence.track(video).unwrap().clips.len(), 1);
}

#[test]
fn 修剪起点让素材入点跟着走() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let id = push(&mut sequence, track, s(10), s(15), s(0));

    let next = applied(&sequence, EditOp::TrimStart { clip: id, to: s(2) });
    let clip = next.clip(id).unwrap();
    assert_eq!(clip.start, s(2));
    assert_eq!(clip.source_in, s(12));
    assert_eq!(clip.source_out, s(15));
    assert_eq!(clip.duration(), s(3));

    assert_eq!(
        apply(&sequence, &EditOp::TrimStart { clip: id, to: s(5) }),
        Err(EditError::EmptyClip),
        "拉到出点上就是空片段"
    );
    assert_eq!(
        apply(
            &sequence,
            &EditOp::TrimStart {
                clip: id,
                to: Tick::from_micros(-1)
            }
        ),
        Err(EditError::NegativeTime)
    );
}

#[test]
fn 修剪撞到邻居时拒绝() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let first = push(&mut sequence, track, s(0), s(2), s(0));
    push(&mut sequence, track, s(5), s(7), s(2));

    assert_eq!(
        apply(
            &sequence,
            &EditOp::TrimStart {
                clip: first,
                to: s(3)
            }
        ),
        Err(EditError::EmptyClip),
        "越过自己的出点"
    );

    // 把第二段的起点往前拉，素材入点还够，但会压住第一段。
    let second = sequence.tracks[0].clips[1].id;
    assert_eq!(
        apply(
            &sequence,
            &EditOp::TrimStart {
                clip: second,
                to: s(1)
            }
        ),
        Err(EditError::Overlap(track))
    );
}

#[test]
fn 修剪终点让素材出点跟着走() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let id = push(&mut sequence, track, s(10), s(15), s(0));

    let next = applied(&sequence, EditOp::TrimEnd { clip: id, to: s(3) });
    let clip = next.clip(id).unwrap();
    assert_eq!(clip.start, s(0));
    assert_eq!(clip.source_in, s(10));
    assert_eq!(clip.source_out, s(13));

    assert_eq!(
        apply(&sequence, &EditOp::TrimEnd { clip: id, to: s(0) }),
        Err(EditError::EmptyClip)
    );
}

#[test]
fn 分割后两段素材时间连续() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let id = push(&mut sequence, track, s(10), s(20), s(0));

    let next = applied(&sequence, EditOp::Split { clip: id, at: s(4) });
    let clips = &next.tracks[0].clips;
    assert_eq!(clips.len(), 2);
    assert_eq!(clips[0].id, id, "左半段保留原 id");
    assert_eq!(clips[0].source_out, clips[1].source_in, "出点接上入点");
    assert_eq!(clips[0].source_in, s(10));
    assert_eq!(clips[0].source_out, s(14));
    assert_eq!(clips[1].start, s(4));
    assert_eq!(clips[1].source_out, s(20));
    assert_eq!(next.duration(), sequence.duration(), "切一刀不改总时长");

    assert_eq!(
        apply(&sequence, &EditOp::Split { clip: id, at: s(0) }),
        Err(EditError::SplitOutside(id)),
        "切在起点不算分割"
    );
    assert_eq!(
        apply(
            &sequence,
            &EditOp::Split {
                clip: id,
                at: s(10)
            }
        ),
        Err(EditError::SplitOutside(id))
    );
}

#[test]
fn 删除留下空隙而波纹删除补齐() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    push(&mut sequence, track, s(0), s(2), s(0));
    let middle = push(&mut sequence, track, s(0), s(2), s(2));
    push(&mut sequence, track, s(0), s(2), s(4));

    let kept = applied(
        &sequence,
        EditOp::Remove {
            clips: vec![middle],
        },
    );
    assert_eq!(starts(&kept, 0), [s(0), s(4)], "普通删除不动别人");

    let rippled = applied(
        &sequence,
        EditOp::RippleDelete {
            clips: vec![middle],
        },
    );
    assert_eq!(starts(&rippled, 0), [s(0), s(2)], "波纹删除把后面的往前补");
    assert_eq!(rippled.duration(), s(4));
}

#[test]
fn 波纹删除只作用于片段所在轨道() {
    let mut sequence = seq();
    let video = sequence.tracks[0].id;
    let audio = sequence.tracks[1].id;
    push(&mut sequence, video, s(0), s(2), s(0));
    let middle = push(&mut sequence, video, s(0), s(2), s(2));
    push(&mut sequence, video, s(0), s(2), s(4));
    push(&mut sequence, audio, s(0), s(6), s(0));

    let next = applied(
        &sequence,
        EditOp::RippleDelete {
            clips: vec![middle],
        },
    );
    assert_eq!(starts(&next, 0), [s(0), s(2)]);
    assert_eq!(
        starts(&next, 1),
        [s(0)],
        "另一条轨道有自己的空隙，不跟着往前推"
    );
}

#[test]
fn 波纹删除多段时按累计时长前移() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let first = push(&mut sequence, track, s(0), s(1), s(0));
    push(&mut sequence, track, s(0), s(1), s(1));
    let third = push(&mut sequence, track, s(0), s(1), s(2));
    push(&mut sequence, track, s(0), s(1), s(3));

    let next = applied(
        &sequence,
        EditOp::RippleDelete {
            clips: vec![first, third],
        },
    );
    assert_eq!(starts(&next, 0), [s(0), s(1)]);
    assert_eq!(next.duration(), s(2));
}

#[test]
fn 滑移只改素材内容() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let id = push(&mut sequence, track, s(10), s(15), s(5));

    let next = applied(&sequence, EditOp::Slip { clip: id, by: s(2) });
    let clip = next.clip(id).unwrap();
    assert_eq!(
        (clip.start, clip.duration()),
        (s(5), s(5)),
        "位置与时长不变"
    );
    assert_eq!((clip.source_in, clip.source_out), (s(12), s(17)));

    let next = applied(
        &next,
        EditOp::Slip {
            clip: id,
            by: s(-3),
        },
    );
    let clip = next.clip(id).unwrap();
    assert_eq!((clip.source_in, clip.source_out), (s(9), s(14)));

    assert_eq!(
        apply(
            &sequence,
            &EditOp::Slip {
                clip: id,
                by: s(-11)
            }
        ),
        Err(EditError::NegativeTime),
        "素材入点不能滑到负数"
    );
}

#[test]
fn 滑动同时修剪两侧邻居() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    push(&mut sequence, track, s(10), s(12), s(0));
    let middle = push(&mut sequence, track, s(20), s(22), s(2));
    push(&mut sequence, track, s(30), s(32), s(4));

    let next = applied(
        &sequence,
        EditOp::Slide {
            clip: middle,
            by: s(1),
        },
    );
    let clips = &next.tracks[0].clips;
    assert_eq!(clips[0].source_out, s(13), "前一段多留一秒");
    assert_eq!(clips[1].start, s(3));
    assert_eq!(clips[1].source_in, s(20), "中间段的素材不变");
    assert_eq!(clips[2].start, s(5));
    assert_eq!(clips[2].source_in, s(31), "后一段让出一秒");
    assert_eq!(clips[2].end(), s(6), "整条轨道的终点不动");

    let next = applied(
        &next,
        EditOp::Slide {
            clip: middle,
            by: s(-1),
        },
    );
    assert_eq!(
        next.tracks[0]
            .clips
            .iter()
            .map(|clip| (clip.start, clip.source_in, clip.source_out))
            .collect::<Vec<_>>(),
        vec![
            (s(0), s(10), s(12)),
            (s(2), s(20), s(22)),
            (s(4), s(30), s(32))
        ],
        "反向滑回来"
    );

    assert_eq!(
        apply(
            &sequence,
            &EditOp::Slide {
                clip: middle,
                by: s(2)
            }
        ),
        Err(EditError::EmptyClip)
    );
}

#[test]
fn 滑移类操作缺少邻居时拒绝() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let first = push(&mut sequence, track, s(0), s(2), s(0));

    assert_eq!(
        apply(
            &sequence,
            &EditOp::Slide {
                clip: first,
                by: s(1)
            }
        ),
        Err(EditError::NotAdjacent)
    );
    assert_eq!(
        apply(
            &sequence,
            &EditOp::Roll {
                clip: first,
                by: s(1)
            }
        ),
        Err(EditError::NotAdjacent)
    );

    // 有片段但中间隔着空隙，同样没有明确语义。
    let gap = push(&mut sequence, track, s(0), s(2), s(4));
    assert_eq!(
        apply(
            &sequence,
            &EditOp::Roll {
                clip: first,
                by: s(1)
            }
        ),
        Err(EditError::NotAdjacent)
    );
    assert_eq!(
        apply(
            &sequence,
            &EditOp::Slide {
                clip: gap,
                by: s(1)
            }
        ),
        Err(EditError::NotAdjacent)
    );
}

#[test]
fn 滚动只移动接缝() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let first = push(&mut sequence, track, s(0), s(2), s(0));
    push(&mut sequence, track, s(10), s(12), s(2));

    let next = applied(
        &sequence,
        EditOp::Roll {
            clip: first,
            by: s(1),
        },
    );
    let clips = &next.tracks[0].clips;
    assert_eq!((clips[0].start, clips[0].source_out), (s(0), s(3)));
    assert_eq!((clips[1].start, clips[1].source_in), (s(3), s(11)));
    assert_eq!(clips[1].end(), s(4), "后一段的终点仍在原处");

    let next = applied(
        &sequence,
        EditOp::Roll {
            clip: first,
            by: s(-1),
        },
    );
    let clips = &next.tracks[0].clips;
    assert_eq!((clips[0].source_out, clips[0].duration()), (s(1), s(1)));
    assert_eq!((clips[1].start, clips[1].source_in), (s(1), s(9)));

    assert_eq!(
        apply(
            &sequence,
            &EditOp::Roll {
                clip: first,
                by: s(2)
            }
        ),
        Err(EditError::EmptyClip),
        "把后一段吃干净"
    );
}

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

// 放置语义（覆盖 / 插入 / 波纹删除）单开一个文件，见 `tests/placement.rs`。
// `#[path]` 是必须的：`tests.rs` 不是 crate root，`mod` 默认只在本目录下找。
#[path = "tests/placement.rs"]
mod placement;

// 变速与定格走 `EditOp` 之后的前置校验，见 `tests/speed.rs`。
#[path = "tests/speed.rs"]
mod speed;

// 调色走 `EditOp` 之后的前置校验，见 `tests/color.rs`。
#[path = "tests/color.rs"]
mod color;

// 字幕条目的增删改，见 `tests/subtitle.rs`。
#[path = "tests/subtitle.rs"]
mod subtitle;
