//! 编辑操作：输入旧序列、输出新序列。
//!
//! 每个操作都是纯函数（[`apply`]）：不改调用方的 `Sequence`，失败时返回错误而不是
//! panic。这样撤销栈可以直接存操作或快照，测试也不需要搭窗口。
//!
//! 两条容易踩空的语义，先写在这里：
//!
//! - **波纹删除只作用于片段所在的轨道**。多条轨道各自有独立的空隙，跨轨一起往前
//!   补会把别的轨道上的片段推进它邻居的怀里。需要整体对齐时由界面层逐轨发操作。
//! - **滑移/滑动/滚动需要相邻片段紧挨着**（前一段的出点等于后一段的入点）。有空隙
//!   时这些操作没有明确语义，直接拒绝，不去猜。
//!
//! 素材长度不在这里校验：片段只知道 `AssetId`，素材时长要等 M1-T07 登记进工程。
//! 这里只保证「素材入点不为负、时长为正、同轨不重叠」这三件模型能自己判断的事。

use crate::timeline::{AssetId, ClipItem, Id, Sequence, Tick};
use std::collections::HashSet;
use std::fmt;

/// 新建片段需要的素材信息。素材入出点都在素材自己的时间轴上。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipSpec {
    pub asset_id: AssetId,
    pub source_in: Tick,
    pub source_out: Tick,
    pub name: String,
}

impl ClipSpec {
    pub fn new(asset_id: AssetId, source_in: Tick, source_out: Tick) -> Self {
        Self {
            asset_id,
            source_in,
            source_out,
            name: String::new(),
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
}

/// 一次编辑。撤销栈按操作粒度记录，一次拖拽对应一个 `Move`。
#[derive(Clone, Debug, PartialEq)]
pub enum EditOp {
    /// 在 `track` 的 `at` 处插入一段素材。
    Insert { track: Id, at: Tick, clip: ClipSpec },
    /// 换轨并挪到新起点。`to_track` 与片段当前轨道类型必须一致。
    Move { clip: Id, to_track: Id, start: Tick },
    /// 把片段起点拉到 `to`，素材入点跟着走，时长相应变化。
    TrimStart { clip: Id, to: Tick },
    /// 把片段终点拉到 `to`，素材出点跟着走。
    TrimEnd { clip: Id, to: Tick },
    /// 在序列时间 `at` 处切成两段。左半段保留原 id，右半段拿新 id。
    Split { clip: Id, at: Tick },
    /// 删除片段，留下空隙。
    Remove { clips: Vec<Id> },
    /// 删除片段并把同轨后面的内容往前补。
    RippleDelete { clips: Vec<Id> },
    /// 素材内容前后滑动，片段在序列上的位置与时长不变。
    Slip { clip: Id, by: Tick },
    /// 片段在时间线上移动，两侧邻居分别伸长 / 缩短同样的量。
    Slide { clip: Id, by: Tick },
    /// 移动片段与后一个片段之间的接缝。
    Roll { clip: Id, by: Tick },
}

/// 编辑被拒绝的原因。文案由这里给出，界面只负责显示。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditError {
    UnknownClip(Id),
    UnknownTrack(Id),
    /// 起点、素材入点或落点为负。
    NegativeTime,
    /// 时长被修成 0 或负数。
    EmptyClip,
    /// 目标轨道上已有片段压住了落点。
    Overlap(Id),
    /// 片段被拖到视频 / 音频 / 字幕之外的轨道类型上。
    WrongTrackKind,
    /// 滑移类操作缺少紧挨着的邻居。
    NotAdjacent,
    /// 切点不在片段内部。
    SplitOutside(Id),
    /// 轨道被锁定，不接受编辑。
    TrackLocked(Id),
}

impl fmt::Display for EditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EditError::UnknownClip(id) => write!(f, "片段 {id} 不在序列里"),
            EditError::UnknownTrack(id) => write!(f, "轨道 {id} 不在序列里"),
            EditError::NegativeTime => write!(f, "时间不能为负"),
            EditError::EmptyClip => write!(f, "片段时长必须大于 0"),
            EditError::Overlap(track) => write!(f, "和轨道 {track} 上已有的片段重叠"),
            EditError::WrongTrackKind => write!(f, "片段不能放到不同类型的轨道上"),
            EditError::NotAdjacent => write!(f, "片段两侧没有紧挨着的邻居，无法滑动"),
            EditError::SplitOutside(id) => write!(f, "切点必须落在片段 {id} 内部"),
            EditError::TrackLocked(id) => write!(f, "轨道 {id} 已锁定，先解锁再编辑"),
        }
    }
}

impl std::error::Error for EditError {}

/// 对序列应用一次编辑，返回新序列；失败时原序列原封不动。
pub fn apply(sequence: &Sequence, op: &EditOp) -> Result<Sequence, EditError> {
    let mut next = sequence.clone();
    match op {
        EditOp::Insert { track, at, clip } => insert(&mut next, *track, *at, clip)?,
        EditOp::Move {
            clip,
            to_track,
            start,
        } => move_clip(&mut next, *clip, *to_track, *start)?,
        EditOp::TrimStart { clip, to } => trim_start(&mut next, *clip, *to)?,
        EditOp::TrimEnd { clip, to } => trim_end(&mut next, *clip, *to)?,
        EditOp::Split { clip, at } => split(&mut next, *clip, *at)?,
        EditOp::Remove { clips } => remove(&mut next, clips)?,
        EditOp::RippleDelete { clips } => ripple_delete(&mut next, clips)?,
        EditOp::Slip { clip, by } => slip(&mut next, *clip, *by)?,
        EditOp::Slide { clip, by } => slide(&mut next, *clip, *by)?,
        EditOp::Roll { clip, by } => roll(&mut next, *clip, *by)?,
    }
    Ok(next)
}

/// 删除若干片段。空列表返回 `None`：这不是错误，界面据此提示「没有选中片段」。
pub fn remove_clips(sequence: &Sequence, clips: &[Id]) -> Result<Option<Sequence>, EditError> {
    if clips.is_empty() {
        return Ok(None);
    }
    apply(
        sequence,
        &EditOp::Remove {
            clips: clips.to_vec(),
        },
    )
    .map(Some)
}

/// 在序列时间 `at` 处切开所有未锁轨道上跨过这一点的片段。
///
/// 返回切完的新序列与刀数。播放头停在空白处、或者正压在片段边界上时返回 `None`：
/// 这不是错误，界面据此提示「这里没有可切的内容」，而不是弹一条错误。
pub fn split_at(sequence: &Sequence, at: Tick) -> Result<Option<(Sequence, usize)>, EditError> {
    let targets: Vec<Id> = sequence
        .tracks
        .iter()
        // 锁定的轨道整条跳过：用户锁它就是不想让它被动。
        .filter(|track| !track.locked)
        .flat_map(|track| track.clips.iter())
        .filter(|clip| clip.start < at && at < clip.end())
        .map(|clip| clip.id)
        .collect();
    if targets.is_empty() {
        return Ok(None);
    }

    let mut next = sequence.clone();
    for clip in &targets {
        next = apply(&next, &EditOp::Split { clip: *clip, at })?;
    }
    Ok(Some((next, targets.len())))
}

/// 找到片段所在的（轨道下标，片段下标）。
///
/// 锁定轨道的片段在这里就拒绝：所有按片段定位的操作都走这个入口，锁定一处生效一片。
fn locate(sequence: &Sequence, clip: Id) -> Result<(usize, usize), EditError> {
    let (track_index, clip_index) = sequence
        .tracks
        .iter()
        .enumerate()
        .find_map(|(track_index, track)| {
            track
                .clip_index(clip)
                .map(|clip_index| (track_index, clip_index))
        })
        .ok_or(EditError::UnknownClip(clip))?;
    if sequence.tracks[track_index].locked {
        return Err(EditError::TrackLocked(sequence.tracks[track_index].id));
    }
    Ok((track_index, clip_index))
}

fn insert(
    sequence: &mut Sequence,
    track_id: Id,
    at: Tick,
    spec: &ClipSpec,
) -> Result<(), EditError> {
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
    let duration = spec.source_out - spec.source_in;
    if sequence.tracks[track_index]
        .overlap(at, at + duration, None)
        .is_some()
    {
        return Err(EditError::Overlap(track_id));
    }

    let id = sequence.alloc_id();
    let clip = ClipItem::new(id, spec.asset_id, spec.source_in, spec.source_out, at)
        .with_name(spec.name.clone());
    let track = &mut sequence.tracks[track_index];
    track.clips.push(clip);
    track.sort_clips();
    Ok(())
}

fn move_clip(
    sequence: &mut Sequence,
    clip_id: Id,
    to_track: Id,
    start: Tick,
) -> Result<(), EditError> {
    if start.is_negative() {
        return Err(EditError::NegativeTime);
    }
    let (from_track, from_index) = locate(sequence, clip_id)?;
    let target = sequence
        .track_index(to_track)
        .ok_or(EditError::UnknownTrack(to_track))?;

    if sequence.tracks[target].kind != sequence.tracks[from_track].kind {
        return Err(EditError::WrongTrackKind);
    }
    if sequence.tracks[target].locked {
        return Err(EditError::TrackLocked(to_track));
    }

    // 先摘下来再检查落点：片段自己不应该被算成「前面的障碍」。
    let mut clip = sequence.tracks[from_track].clips.remove(from_index);
    let end = start + clip.duration();
    if sequence.tracks[target].overlap(start, end, None).is_some() {
        return Err(EditError::Overlap(to_track));
    }

    clip.start = start;
    let track = &mut sequence.tracks[target];
    track.clips.push(clip);
    track.sort_clips();
    Ok(())
}

fn trim_start(sequence: &mut Sequence, clip_id: Id, to: Tick) -> Result<(), EditError> {
    if to.is_negative() {
        return Err(EditError::NegativeTime);
    }
    let (track_index, clip_index) = locate(sequence, clip_id)?;
    let track_id = sequence.tracks[track_index].id;
    let clip = &sequence.tracks[track_index].clips[clip_index];

    let delta = to - clip.start;
    let source_in = clip.source_in + delta;
    let end = clip.end();
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
    clip.start = to;
    clip.source_in = source_in;
    Ok(())
}

fn trim_end(sequence: &mut Sequence, clip_id: Id, to: Tick) -> Result<(), EditError> {
    let (track_index, clip_index) = locate(sequence, clip_id)?;
    let track_id = sequence.tracks[track_index].id;
    let clip = &sequence.tracks[track_index].clips[clip_index];

    let source_out = clip.source_in + (to - clip.start);
    if to <= clip.start || source_out <= clip.source_in {
        return Err(EditError::EmptyClip);
    }
    if sequence.tracks[track_index]
        .overlap(clip.start, to, Some(clip_id))
        .is_some()
    {
        return Err(EditError::Overlap(track_id));
    }

    sequence.tracks[track_index].clips[clip_index].source_out = source_out;
    Ok(())
}

fn split(sequence: &mut Sequence, clip_id: Id, at: Tick) -> Result<(), EditError> {
    let (track_index, clip_index) = locate(sequence, clip_id)?;
    let id = sequence.alloc_id();
    let track = &mut sequence.tracks[track_index];
    let clip = track.clips[clip_index].clone();

    if at <= clip.start || at >= clip.end() {
        return Err(EditError::SplitOutside(clip_id));
    }

    // 两段的素材时间严丝合缝：左段出点就是右段入点，不丢帧也不重复。
    let source_cut = clip.source_in + (at - clip.start);
    track.clips[clip_index].source_out = source_cut;
    track.clips.push(
        ClipItem::new(id, clip.asset_id, source_cut, clip.source_out, at).with_name(clip.name),
    );
    track.sort_clips();
    Ok(())
}

fn remove(sequence: &mut Sequence, clips: &[Id]) -> Result<(), EditError> {
    let ids = checked_ids(sequence, clips)?;
    for track in &mut sequence.tracks {
        // 转场引用了被删的片段，就没有存在的意义了；留着会让工程校验失败。
        track.transitions.retain(|transition| {
            !ids.contains(&transition.before) && !ids.contains(&transition.after)
        });
        track.clips.retain(|clip| !ids.contains(&clip.id));
    }
    Ok(())
}

fn ripple_delete(sequence: &mut Sequence, clips: &[Id]) -> Result<(), EditError> {
    let ids = checked_ids(sequence, clips)?;
    for track in &mut sequence.tracks {
        let removed: Vec<(Tick, Tick)> = track
            .clips
            .iter()
            .filter(|clip| ids.contains(&clip.id))
            .map(|clip| (clip.start, clip.end()))
            .collect();
        if removed.is_empty() {
            continue;
        }

        track.transitions.retain(|transition| {
            !ids.contains(&transition.before) && !ids.contains(&transition.after)
        });
        track.clips.retain(|clip| !ids.contains(&clip.id));

        for clip in &mut track.clips {
            // 每个片段只被「完全排在它前面的」被删片段推着走；两者不重叠，
            // 所以这个和一定小于等于它自己的起点，不会把时间推成负数。
            let shift = removed
                .iter()
                .filter(|(start, _)| *start < clip.start)
                .fold(Tick::ZERO, |total, (start, end)| total + (*end - *start));
            clip.start = clip.start - shift;
        }
        track.sort_clips();
    }
    Ok(())
}

fn slip(sequence: &mut Sequence, clip_id: Id, by: Tick) -> Result<(), EditError> {
    let (track_index, clip_index) = locate(sequence, clip_id)?;
    let clip = &sequence.tracks[track_index].clips[clip_index];
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

fn slide(sequence: &mut Sequence, clip_id: Id, by: Tick) -> Result<(), EditError> {
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

fn roll(sequence: &mut Sequence, clip_id: Id, by: Tick) -> Result<(), EditError> {
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

/// 去重并确认每个 id 都还在序列里。重复 id 不算错误：界面多选时可能没去重。
fn checked_ids(sequence: &Sequence, clips: &[Id]) -> Result<HashSet<Id>, EditError> {
    let mut ids = HashSet::new();
    for id in clips {
        // `locate` 负责「在不在」与「所在轨道锁没锁」两件事，不另写一套。
        locate(sequence, *id)?;
        ids.insert(*id);
    }
    Ok(ids)
}

#[cfg(test)]
mod tests;
