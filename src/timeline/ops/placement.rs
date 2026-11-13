//! 把素材放到轨道上：严格插入、插入（后段让开）、覆盖（清掉区间）。
//!
//! 三者共用 [`prepare_placement`] 的校验与 [`place`] 的落位，区别只在「落点原有内容怎么办」：
//! 严格插入不许压到东西，插入把后段整体推开，覆盖把区间里的内容清掉再放。
//!
//! 只有 `ops::apply` 与同层的移动类操作会调到这里，所以对外一律 `pub(super)`。

use super::{ClipSpec, EditError, Id, Sequence, Tick, cut_at, split};
use crate::timeline::{ClipItem, SpeedCurve};
use std::collections::HashSet;

pub(super) fn insert(
    sequence: &mut Sequence,
    track_id: Id,
    at: Tick,
    spec: &ClipSpec,
) -> Result<(), EditError> {
    let (track_index, duration) = prepare_placement(sequence, track_id, at, spec)?;
    if sequence.tracks[track_index]
        .overlap(at, at + duration, None)
        .is_some()
    {
        return Err(EditError::Overlap(track_id));
    }
    place(sequence, track_index, at, spec);
    Ok(())
}

/// 插入：在落点把后面的内容整体让开，再放进去。
pub(super) fn insert_at(
    sequence: &mut Sequence,
    track_id: Id,
    at: Tick,
    spec: &ClipSpec,
) -> Result<(), EditError> {
    let (track_index, duration) = prepare_placement(sequence, track_id, at, spec)?;
    shift_open(sequence, track_index, at, duration);
    place(sequence, track_index, at, spec);
    Ok(())
}

/// 覆盖：先清掉落点区间里的内容，再放进去。不产生重叠片段。
pub(super) fn overwrite(
    sequence: &mut Sequence,
    track_id: Id,
    at: Tick,
    spec: &ClipSpec,
) -> Result<(), EditError> {
    let (track_index, duration) = prepare_placement(sequence, track_id, at, spec)?;
    clear_range(sequence, track_index, at, at + duration);
    place(sequence, track_index, at, spec);
    Ok(())
}

/// 放素材前的公共校验：时长与落点合法、轨道存在且没锁。返回轨道下标与素材时长。
fn prepare_placement(
    sequence: &Sequence,
    track_id: Id,
    at: Tick,
    spec: &ClipSpec,
) -> Result<(usize, Tick), EditError> {
    if spec.source_out <= spec.source_in {
        return Err(EditError::EmptyClip);
    }
    if at.is_negative() {
        return Err(EditError::NegativeTime);
    }
    let track_index = sequence
        .track_index(track_id)
        .ok_or(EditError::UnknownTrack(track_id))?;
    if sequence.tracks[track_index].locked {
        return Err(EditError::TrackLocked(track_id));
    }
    Ok((track_index, spec.source_out - spec.source_in))
}

/// 把新片段放到 `at`。调用方负责已经腾好位置与校验过轨道。
fn place(sequence: &mut Sequence, track_index: usize, at: Tick, spec: &ClipSpec) {
    let id = sequence.alloc_id();
    let clip = ClipItem::new(id, spec.asset_id, spec.source_in, spec.source_out, at)
        .with_name(spec.name.clone());
    let track = &mut sequence.tracks[track_index];
    track.clips.push(clip);
    track.sort_clips();
}

/// 在 `at` 处让开 `duration`：跨过落点的片段先切开，再把 `at` 及之后的整体右移。
///
/// 切开这一步是必要的：只右移的话，跨过落点的片段会被新素材压住一截，
/// 而片段是原子对象，没法「只挪后半段」。
pub(super) fn shift_open(sequence: &mut Sequence, track_index: usize, at: Tick, duration: Tick) {
    let splits: Vec<Id> = sequence.tracks[track_index]
        .clips
        .iter()
        .filter(|clip| clip.start < at && at < clip.end())
        .map(|clip| clip.id)
        .collect();
    for id in splits {
        // `split` 会再查一次锁定；这里已经查过，失败只能是并发改序列，忽略即可。
        let _ = split(sequence, id, at);
    }
    for clip in &mut sequence.tracks[track_index].clips {
        if clip.start >= at {
            clip.start = clip.start + duration;
        }
    }
    sequence.tracks[track_index].sort_clips();
}

/// 让出 `[at, end)`：完全被盖住的删掉，压住两端的收边或切开。
///
/// 四种情形分开处理，别指望「先删后补」能省事——被切开的那一段要拿新 id，
/// 素材入点也得跟着落点走，补不回来。
pub(super) fn clear_range(sequence: &mut Sequence, track_index: usize, at: Tick, end: Tick) {
    let originals: Vec<ClipItem> = sequence.tracks[track_index].clips.clone();
    let mut remove: Vec<Id> = Vec::new();
    let mut trim: Vec<(Id, Tick, SpeedCurve)> = Vec::new();
    let mut shift: Vec<(Id, Tick, Tick, SpeedCurve)> = Vec::new();
    let mut created: Vec<ClipItem> = Vec::new();

    for clip in &originals {
        let (start, stop) = (clip.start, clip.end());
        if stop <= at || start >= end {
            continue;
        }
        // 收边 / 切开都要按**曲线**反解素材时间，曲线也跟着切：1:1 的换算只对
        // 常量倍率成立，曲线片段要是照 1:1 算，切出来的两段时长都会跑偏。
        match (start < at, stop > end) {
            // 整段都在区间里：删掉。
            (false, false) => remove.push(clip.id),
            // 只被压住右半：出点收到 `at`，素材出点跟着走。
            (true, false) => {
                let (source, left, _) = cut_at(clip, at);
                trim.push((clip.id, source, left));
            }
            // 只被压住左半：起点拉到 `end`，素材入点跟着走。
            (false, true) => {
                let (source, _, right) = cut_at(clip, end);
                shift.push((clip.id, end, source, right));
            }
            // 区间正压在中间：左边留下，右边切一段新的出来。
            (true, true) => {
                let (cut, left, _) = cut_at(clip, at);
                trim.push((clip.id, cut, left));
                let (resume, _, right) = cut_at(clip, end);
                let mut piece = ClipItem::new(
                    sequence.alloc_id(),
                    clip.asset_id,
                    resume,
                    clip.source_out,
                    end,
                )
                .with_name(clip.name.clone());
                piece.remap = right;
                created.push(piece);
            }
        }
    }

    let track = &mut sequence.tracks[track_index];
    for (id, source_out, curve) in trim {
        if let Some(clip) = track.clips.iter_mut().find(|clip| clip.id == id) {
            clip.source_out = source_out;
            clip.remap = curve;
        }
    }
    for (id, start, source_in, curve) in shift {
        if let Some(clip) = track.clips.iter_mut().find(|clip| clip.id == id) {
            clip.start = start;
            clip.source_in = source_in;
            clip.remap = curve;
        }
    }
    let gone: HashSet<Id> = remove.iter().copied().collect();
    track.clips.retain(|clip| !gone.contains(&clip.id));
    track.transitions.retain(|transition| {
        !gone.contains(&transition.before) && !gone.contains(&transition.after)
    });
    track.clips.extend(created);
    track.sort_clips();
}
