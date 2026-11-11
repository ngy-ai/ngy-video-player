//! 字幕增删改走 `EditOp` 的前置校验：锁轨、类型、重叠与 id 冲突。

use super::*;
use crate::timeline::{COLORS, MAX_SIZE, SubtitlePosition};

fn entry(id: Id, start: i64, end: i64, text: &str) -> SubtitleItem {
    SubtitleItem::new(id, s(start), s(end), text)
}

fn lane(sequence: &mut Sequence) -> Id {
    sequence.add_track(TrackKind::Subtitle)
}

#[test]
fn 加字幕按起点排序并修好_id_计数器() {
    let mut sequence = seq();
    let track = lane(&mut sequence);
    let first = Id::new(900);
    let second = Id::new(901);
    let next = applied(
        &sequence,
        EditOp::AddSubtitle {
            track,
            item: entry(first, 3, 5, "后一句"),
        },
    );
    let next = applied(
        &next,
        EditOp::AddSubtitle {
            track,
            item: entry(second, 0, 2, "前一句"),
        },
    );
    let items = &next.track(track).unwrap().subtitles;
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, second, "按起点排序，不是按添加顺序");
    assert_eq!(items[0].text, "前一句");
    let mut probe = next.clone();
    assert!(probe.alloc_id().raw() > 901, "计数器已经推过手写的 901");
}

#[test]
fn 加字幕规整坏值并拒绝重叠与坏区间() {
    let mut sequence = seq();
    let track = lane(&mut sequence);
    let id = sequence.alloc_id();
    let mut bad = entry(id, 1, 1, "  ");
    bad.style.size = 999;
    let next = applied(&sequence, EditOp::AddSubtitle { track, item: bad });
    let added = next.subtitle(id).unwrap();
    assert_eq!(added.text, "新字幕");
    assert_eq!(added.end, s(4), "空区间兜成 3 秒");
    assert_eq!(added.style.size, MAX_SIZE);

    let overlap = entry(sequence.alloc_id(), 2, 3, "压住了");
    assert_eq!(
        apply(
            &next,
            &EditOp::AddSubtitle {
                track,
                item: overlap
            }
        ),
        Err(EditError::Overlap(track))
    );
    assert_eq!(
        apply(
            &next,
            &EditOp::AddSubtitle {
                track,
                item: entry(sequence.alloc_id(), -1, 1, "负起点")
            }
        ),
        Err(EditError::NegativeTime)
    );
    // 首尾相接不算重叠。
    let touching = entry(sequence.alloc_id(), 4, 6, "接着上一句");
    let next = applied(
        &next,
        EditOp::AddSubtitle {
            track,
            item: touching,
        },
    );
    assert_eq!(next.track(track).unwrap().subtitles.len(), 2);
}

#[test]
fn 字幕只进字幕轨且锁轨与占用_id_都拒绝() {
    let mut sequence = seq();
    let video = sequence.tracks[0].id;
    let track = lane(&mut sequence);
    let id = sequence.alloc_id();
    let item = entry(id, 0, 1, "文本");

    assert_eq!(
        apply(
            &sequence,
            &EditOp::AddSubtitle {
                track: video,
                item: item.clone()
            }
        ),
        Err(EditError::WrongTrackKind)
    );
    assert_eq!(
        apply(
            &sequence,
            &EditOp::AddSubtitle {
                track: Id::new(999),
                item: item.clone()
            }
        ),
        Err(EditError::UnknownTrack(Id::new(999)))
    );
    assert_eq!(
        apply(
            &sequence,
            &EditOp::AddSubtitle {
                track,
                item: entry(video, 0, 1, "撞轨道 id")
            }
        ),
        Err(EditError::DuplicateId(video))
    );
    sequence.track_mut(track).unwrap().locked = true;
    assert_eq!(
        apply(&sequence, &EditOp::AddSubtitle { track, item }),
        Err(EditError::TrackLocked(track))
    );
}

#[test]
fn 改字幕保留_id_且没变化时不产生新序列() {
    let mut sequence = seq();
    let track = lane(&mut sequence);
    let id = sequence.alloc_id();
    let sequence = applied(
        &sequence,
        EditOp::AddSubtitle {
            track,
            item: entry(id, 0, 2, "原文"),
        },
    );

    let mut changed = entry(Id::new(777), 0, 2, "改过的文字");
    changed.style.color = COLORS[1].0;
    changed.style.position = SubtitlePosition::Top;
    let next = applied(&sequence, EditOp::SetSubtitle { id, item: changed });
    let item = next.subtitle(id).unwrap();
    assert_eq!(item.id, id, "面板不能顺手把 id 换掉");
    assert_eq!(item.text, "改过的文字");
    assert_eq!(item.style.color, COLORS[1].0);
    assert_eq!(item.style.position, SubtitlePosition::Top);

    let same = next.subtitle(id).unwrap().clone();
    assert_eq!(
        apply(&next, &EditOp::SetSubtitle { id, item: same }).unwrap(),
        next,
        "值没变时序列逐字段相等，界面据此不落撤销"
    );
    assert_eq!(
        apply(
            &next,
            &EditOp::SetSubtitle {
                id: Id::new(999),
                item: entry(id, 0, 1, "没有这条")
            }
        ),
        Err(EditError::UnknownSubtitle(Id::new(999)))
    );
}

#[test]
fn 删字幕与删轨都把它带走() {
    let mut sequence = seq();
    let track = lane(&mut sequence);
    let first = sequence.alloc_id();
    let second = sequence.alloc_id();
    let sequence = applied(
        &sequence,
        EditOp::AddSubtitle {
            track,
            item: entry(first, 0, 2, "第一句"),
        },
    );
    let sequence = applied(
        &sequence,
        EditOp::AddSubtitle {
            track,
            item: entry(second, 2, 4, "第二句"),
        },
    );

    let next = applied(&sequence, EditOp::RemoveSubtitle { id: first });
    assert_eq!(next.track(track).unwrap().subtitles.len(), 1);
    assert_eq!(next.track(track).unwrap().subtitles[0].id, second);
    assert_eq!(
        apply(&next, &EditOp::RemoveSubtitle { id: first }),
        Err(EditError::UnknownSubtitle(first))
    );

    let mut locked = next.clone();
    locked.track_mut(track).unwrap().locked = true;
    assert_eq!(
        apply(&locked, &EditOp::RemoveSubtitle { id: second }),
        Err(EditError::TrackLocked(track))
    );

    let empty = applied(&next, EditOp::RemoveTrack { track });
    assert!(empty.subtitle(second).is_none());
    assert!(empty.validate().is_ok());
}
