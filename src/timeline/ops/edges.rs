//! 边界编辑：修剪（普通 / 波纹）、切开、滑移 / 滑动 / 卷动。
//!
//! 这一组操作的共同点是**同时动时间线边界和素材入出点**，所以要过
//! [`cut_at`]：没有速度曲线时是 1:1 的换算，挂了曲线要按曲线反解素材时间、
//! 并把曲线在切点处切片——两段各自只保留自己那半，时长才对得上。
//!
//! 滑移 / 滑动 / 卷动把时间线上的位移直接加到素材时间上，只对常量倍率成立；
//! 曲线片段一律拒绝（[`EditError::CurveShift`]），不去猜一个近似结果。

use super::{EditError, Id, Sequence, Tick, locate};
use crate::timeline::{ClipItem, SpeedCurve};
pub(super) fn trim_start(sequence: &mut Sequence, clip_id: Id, to: Tick) -> Result<(), EditError> {
    if to.is_negative() {
        return Err(EditError::NegativeTime);
    }
    let (track_index, clip_index) = locate(sequence, clip_id)?;
    let track_id = sequence.tracks[track_index].id;
    let clip = &sequence.tracks[track_index].clips[clip_index];

    let delta = to - clip.start;
    let source_in = clip.source_in + delta;
    let end = clip.end();
    // 速度曲线下「往左拉」要借曲线没覆盖的那段素材，表达不出来；只能往里收。
    let curved = clip.has_remap();
    if curved && delta < Tick::ZERO {
        return Err(EditError::CurveShift);
    }
    if source_in.is_negative() {
        return Err(EditError::EmptyClip);
    }
    if to >= end {
        return Err(EditError::EmptyClip);
    }
    if sequence.tracks[track_index]
        .overlap(to, end, Some(clip_id))
        .is_some()
    {
        return Err(EditError::Overlap(track_id));
    }

    let clip = &mut sequence.tracks[track_index].clips[clip_index];
    if curved {
        // 素材入点按曲线反解，曲线也切掉前半段：剩下的那半段时长才对得上。
        let (source, _, right) = cut_at(clip, to);
        clip.source_in = source;
        clip.remap = right;
    } else {
        clip.source_in = source_in;
    }
    clip.start = to;
    Ok(())
}

pub(super) fn trim_end(sequence: &mut Sequence, clip_id: Id, to: Tick) -> Result<(), EditError> {
    let (track_index, clip_index) = locate(sequence, clip_id)?;
    let track_id = sequence.tracks[track_index].id;
    let clip = &sequence.tracks[track_index].clips[clip_index];

    let source_out = clip.source_in + (to - clip.start);
    if to <= clip.start || source_out <= clip.source_in {
        return Err(EditError::EmptyClip);
    }
    // 曲线片段只能往里收：往外拉要借曲线没覆盖的素材。
    let curved = clip.has_remap();
    if curved && to > clip.end() {
        return Err(EditError::CurveShift);
    }
    if sequence.tracks[track_index]
        .overlap(clip.start, to, Some(clip_id))
        .is_some()
    {
        return Err(EditError::Overlap(track_id));
    }

    let clip = &mut sequence.tracks[track_index].clips[clip_index];
    if curved {
        let (source, left, _) = cut_at(clip, to);
        clip.source_out = source;
        clip.remap = left;
    } else {
        clip.source_out = source_out;
    }
    Ok(())
}

/// 波纹修剪头：内容从头部收掉 `to - start`，`start` 不动，同轨后面的片段整体左移。
///
/// 与 [`trim_start`] 的区别只有一处：那边收头会在轨道上留下空隙，这边把空隙闭合。
/// `to` 是「内容收到了哪儿」；负数方向（往左）= 把被剪掉的帧要回来，受素材入点约束。
pub(super) fn ripple_trim_start(
    sequence: &mut Sequence,
    clip_id: Id,
    to: Tick,
) -> Result<(), EditError> {
    let (track_index, clip_index) = locate(sequence, clip_id)?;
    let clip = &sequence.tracks[track_index].clips[clip_index];
    let delta = to - clip.start;
    if delta == Tick::ZERO {
        return Ok(());
    }
    let source_in = clip.source_in + delta;
    let old_end = clip.end();
    if source_in.is_negative() || delta >= old_end - clip.start {
        return Err(EditError::EmptyClip);
    }
    // 曲线片段不能往左借素材（那一段没有速率定义）。
    let curved = clip.has_remap();
    if curved && delta < Tick::ZERO {
        return Err(EditError::CurveShift);
    }

    let clip = &mut sequence.tracks[track_index].clips[clip_index];
    if curved {
        let (source, _, right) = cut_at(clip, to);
        clip.source_in = source;
        clip.remap = right;
    } else {
        clip.source_in = source_in;
    }
    // 头部收放 `delta` 之后终点跟着挪同样的量（与 `trim_start` 同一个 1:1 约定），
    // 后面的片段补上这段差值，轨道上就不会留空隙。
    let shift = Tick::ZERO - delta;
    for other in &mut sequence.tracks[track_index].clips {
        if other.id != clip_id && other.start >= old_end {
            other.start = other.start + shift;
        }
    }
    Ok(())
}

/// 波纹修剪尾：出点收到 `to`，同轨后面的片段整体跟着挪。
///
/// 不检查与下一段重叠——后面的片段本来就要一起让位；`trim_end` 的那条检查在这里
/// 反而会把正常的波纹修剪拒掉。
pub(super) fn ripple_trim_end(
    sequence: &mut Sequence,
    clip_id: Id,
    to: Tick,
) -> Result<(), EditError> {
    let (track_index, clip_index) = locate(sequence, clip_id)?;
    let clip = &sequence.tracks[track_index].clips[clip_index];
    let old_end = clip.end();
    let delta = to - old_end;
    if delta == Tick::ZERO {
        return Ok(());
    }
    let source_out = clip.source_in + (to - clip.start);
    if to <= clip.start || source_out <= clip.source_in {
        return Err(EditError::EmptyClip);
    }
    // 曲线片段只能往里收：往外拉要借曲线没覆盖的素材。
    let curved = clip.has_remap();
    if curved && to > old_end {
        return Err(EditError::CurveShift);
    }

    let clip = &mut sequence.tracks[track_index].clips[clip_index];
    if curved {
        let (source, left, _) = cut_at(clip, to);
        clip.source_out = source;
        clip.remap = left;
    } else {
        clip.source_out = source_out;
    }
    for other in &mut sequence.tracks[track_index].clips {
        if other.id != clip_id && other.start >= old_end {
            other.start = other.start + delta;
        }
    }
    Ok(())
}

/// 在时间线 `at` 处把片段断开：切点的素材时间 + 左右两段各自的曲线。
///
/// 没有曲线时就是 1:1 的换算（返回两条空曲线），有曲线时按 `source_at` 反解，
/// 再把曲线切一刀——两段各自只保留自己那半，时长才不会按整条曲线算。
pub(super) fn cut_at(clip: &ClipItem, at: Tick) -> (Tick, SpeedCurve, SpeedCurve) {
    let source = clip
        .source_at(at)
        .unwrap_or((clip.source_out - Tick::from_micros(1)).max(clip.source_in));
    if !clip.has_remap() {
        return (source, SpeedCurve::default(), SpeedCurve::default());
    }
    let offset = (source - clip.source_in).secs();
    let span = clip.span().secs();
    (
        source,
        clip.remap.slice(0.0, offset),
        clip.remap.slice(offset, span),
    )
}

pub(super) fn split(sequence: &mut Sequence, clip_id: Id, at: Tick) -> Result<(), EditError> {
    let (track_index, clip_index) = locate(sequence, clip_id)?;
    let id = sequence.alloc_id();
    let track = &mut sequence.tracks[track_index];
    let clip = track.clips[clip_index].clone();

    if at <= clip.start || at >= clip.end() {
        return Err(EditError::SplitOutside(clip_id));
    }

    // 两段的素材时间严丝合缝：左段出点就是右段入点，不丢帧也不重复。
    let (source_cut, left, right) = cut_at(&clip, at);
    track.clips[clip_index].source_out = source_cut;
    track.clips[clip_index].remap = left;
    // 右段的起点取左段的**实际**终点：曲线片段的时长是积分出来的，和 `at` 差
    // 一两个微秒，直接用 `at` 会和左段差出一次「重叠」或者一道缝。
    let seam = track.clips[clip_index].end();
    let mut piece =
        ClipItem::new(id, clip.asset_id, source_cut, clip.source_out, seam).with_name(clip.name);
    piece.remap = right;
    track.clips.push(piece);
    track.sort_clips();
    Ok(())
}

/// 滑移 / 滑动 / 卷动的公共守卫：这些操作把时间线上的 `by` 直接加到素材时间上，
/// 只对常量倍率成立。曲线片段要反解曲线、曲线本身也得重新切片，先明确拒绝。
fn guard_curve(clip: &ClipItem) -> Result<(), EditError> {
    if clip.has_remap() {
        return Err(EditError::CurveShift);
    }
    Ok(())
}

pub(super) fn slip(sequence: &mut Sequence, clip_id: Id, by: Tick) -> Result<(), EditError> {
    let (track_index, clip_index) = locate(sequence, clip_id)?;
    let clip = &sequence.tracks[track_index].clips[clip_index];
    guard_curve(clip)?;
    let source_in = clip.source_in + by;
    let source_out = clip.source_out + by;
    if source_in.is_negative() {
        return Err(EditError::NegativeTime);
    }
    if source_out <= source_in {
        return Err(EditError::EmptyClip);
    }

    let clip = &mut sequence.tracks[track_index].clips[clip_index];
    clip.source_in = source_in;
    clip.source_out = source_out;
    Ok(())
}

pub(super) fn slide(sequence: &mut Sequence, clip_id: Id, by: Tick) -> Result<(), EditError> {
    if by == Tick::ZERO {
        return Ok(());
    }
    let (track_index, clip_index) = locate(sequence, clip_id)?;
    let track = &sequence.tracks[track_index];
    let clip = track.clips[clip_index].clone();

    // 滑移要同时吃两侧邻居：左边那段补不补得出来、右边那段让不让得出来，
    // 都得先确认它们真的贴着。
    if clip_index == 0 || clip_index + 1 >= track.clips.len() {
        return Err(EditError::NotAdjacent);
    }
    let before = track.clips[clip_index - 1].clone();
    let after = track.clips[clip_index + 1].clone();
    if before.end() != clip.start || clip.end() != after.start {
        return Err(EditError::NotAdjacent);
    }
    guard_curve(&before)?;
    guard_curve(&clip)?;
    guard_curve(&after)?;

    let source_out = before.source_out + by;
    if source_out <= before.source_in {
        return Err(EditError::EmptyClip);
    }
    let after_in = after.source_in + by;
    if after_in.is_negative() {
        return Err(EditError::NegativeTime);
    }
    if after.source_out <= after_in {
        return Err(EditError::EmptyClip);
    }
    if clip.start + by < Tick::ZERO {
        return Err(EditError::NegativeTime);
    }

    let track = &mut sequence.tracks[track_index];
    track.clips[clip_index - 1].source_out = source_out;
    track.clips[clip_index].start = clip.start + by;
    track.clips[clip_index + 1].source_in = after_in;
    track.clips[clip_index + 1].start = after.start + by;
    Ok(())
}

pub(super) fn roll(sequence: &mut Sequence, clip_id: Id, by: Tick) -> Result<(), EditError> {
    if by == Tick::ZERO {
        return Ok(());
    }
    let (track_index, clip_index) = locate(sequence, clip_id)?;
    let track = &sequence.tracks[track_index];
    let clip = track.clips[clip_index].clone();
    let Some(after) = track.clips.get(clip_index + 1).cloned() else {
        return Err(EditError::NotAdjacent);
    };
    if clip.end() != after.start {
        return Err(EditError::NotAdjacent);
    }
    guard_curve(&clip)?;
    guard_curve(&after)?;

    let source_out = clip.source_out + by;
    if source_out <= clip.source_in {
        return Err(EditError::EmptyClip);
    }
    let after_in = after.source_in + by;
    if after_in.is_negative() {
        return Err(EditError::NegativeTime);
    }
    if after.source_out <= after_in {
        return Err(EditError::EmptyClip);
    }

    let track = &mut sequence.tracks[track_index];
    track.clips[clip_index].source_out = source_out;
    track.clips[clip_index + 1].source_in = after_in;
    track.clips[clip_index + 1].start = after.start + by;
    Ok(())
}
