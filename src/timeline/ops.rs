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

mod edges;
mod params;
mod placement;
mod remove;
mod subtitle;

use crate::timeline::{
    AssetId, ClipItem, ColorGrade, Id, Sequence, SpeedCurve, SubtitleItem, Tick,
};
use edges::{
    cut_at, ripple_trim_end, ripple_trim_start, roll, slide, slip, split, trim_end, trim_start,
};
use params::{set_color, set_hold, set_remap, set_speed};
use placement::{clear_range, insert, insert_at, overwrite, shift_open};
use remove::{remove, ripple_delete};
use std::collections::HashSet;
use std::fmt;
use subtitle::{add_subtitle, remove_subtitle, set_subtitle};

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

/// 落点被已有片段压住时怎么处理。
///
/// 界面层按修饰键选一个：直接拖是覆盖，按住 `Shift` 是插入。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// 覆盖：清掉落点区间里的内容。序列长度不变。
    Overwrite,
    /// 插入：在落点切开后续内容并整体右移。序列变长。
    Insert,
}

/// 一次编辑。撤销栈按操作粒度记录，一次拖拽对应一个 `Move`。
#[derive(Clone, Debug, PartialEq)]
pub enum EditOp {
    /// 在 `track` 的 `at` 处放一段素材。落点被占就拒绝（严格的「空白处放」）。
    Insert { track: Id, at: Tick, clip: ClipSpec },
    /// 插入模式：在 `at` 处切开后续片段、整体右移，再把素材放进去。
    ///
    /// 与 `Insert` 的区别只在落点压到东西时：`Insert` 拒绝，这里把后面的内容让开。
    InsertAt { track: Id, at: Tick, clip: ClipSpec },
    /// 覆盖模式：先清掉 `[at, at+时长)` 里的内容，再把素材放进去。
    Overwrite { track: Id, at: Tick, clip: ClipSpec },
    /// 换轨并挪到新起点。`to_track` 与片段当前轨道类型必须一致；落点被占就拒绝。
    Move { clip: Id, to_track: Id, start: Tick },
    /// 移动片段并用覆盖语义落位：清掉落点区间里原有的内容。
    MoveOverwrite { clip: Id, to_track: Id, start: Tick },
    /// 移动片段并用插入语义落位：在落点切开后续片段并整体右移。
    MoveInsert { clip: Id, to_track: Id, start: Tick },
    /// 把片段起点拉到 `to`，素材入点跟着走，时长相应变化。
    TrimStart { clip: Id, to: Tick },
    /// 把片段终点拉到 `to`，素材出点跟着走。
    TrimEnd { clip: Id, to: Tick },
    /// 波纹修剪头：从入点一侧收掉 `to` 给的那一段，**同轨后面的片段跟着往前补**。
    ///
    /// `to` 是「内容要收到哪儿」（片段内相对序列时间），不是片段的新起点：波纹修剪之后
    /// 片段仍然顶在原来的位置上，长度变短、后面的片段整体左移，轨道上不留空隙。
    RippleTrimStart { clip: Id, to: Tick },
    /// 波纹修剪尾：出点收到 `to`，同轨后面的片段跟着往前补。
    RippleTrimEnd { clip: Id, to: Tick },
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
    /// 改片段倍率。长度按倍率反比变化，同轨后面的片段整体让位（波纹）。
    SetSpeed { clip: Id, speed: f32 },
    /// 换整条速度曲线（时间重映射）。空曲线表示回到常量倍率 `speed`；
    /// 长度按曲线的积分重算，同轨后面的片段同样让位。
    SetRemap { clip: Id, curve: SpeedCurve },
    /// 片尾定格：最后一帧多停 `hold`，`Tick::ZERO` 表示取消。
    SetHold { clip: Id, hold: Tick },
    /// 改调色。整套参数一起换：面板每次改动都是一份完整的 `ColorGrade`，
    /// 单个字段的增删在界面层算好，落一步撤销。
    SetColor { clip: Id, color: ColorGrade },
    /// 把片段内 `from` 处的整组关键帧挪到 `to`（都是片段内相对时间）。
    /// 越界不报错，夹到片段两端；`from` 处没有点时序列原样。
    MoveKeyframe { clip: Id, from: Tick, to: Tick },
    /// 在字幕轨上加一条文字条目。id 由调用方从序列分配，重叠就拒绝。
    AddSubtitle { track: Id, item: SubtitleItem },
    /// 改一条字幕的全部内容（文本、起止、样式）。id 不变。
    SetSubtitle { id: Id, item: SubtitleItem },
    /// 删掉一条字幕。
    RemoveSubtitle { id: Id },
    /// 删掉整条轨道，连同它上面的片段、转场与字幕条目。锁定的轨道要先解锁。
    RemoveTrack { track: Id },
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
    /// 倍率不是有限数，或者定格时长为负。
    BadRate,
    /// 挂了速度曲线的片段不能做「把时间线位移直接加到素材时间上」的编辑：
    /// 滑移 / 滑动 / 卷动，以及把边界往外拉。这些都要反解曲线，先清空曲线再做。
    CurveShift,
    /// 字幕条目的 id 已经被片段 / 轨道 / 另一条字幕占用。
    DuplicateId(Id),
    /// 字幕条目本身不在序列里。
    UnknownSubtitle(Id),
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
            EditError::BadRate => write!(f, "倍率必须是有限数，定格时长不能为负"),
            EditError::CurveShift => write!(
                f,
                "挂了速度曲线的片段不能滑移 / 滑动 / 卷动，也不能把边界往外拉：先清空曲线"
            ),
            EditError::DuplicateId(id) => write!(f, "id {id} 已经被别的内容占用了"),
            EditError::UnknownSubtitle(id) => write!(f, "字幕 {id} 不在序列里"),
        }
    }
}

impl std::error::Error for EditError {}

/// 对序列应用一次编辑，返回新序列；失败时原序列原封不动。
pub fn apply(sequence: &Sequence, op: &EditOp) -> Result<Sequence, EditError> {
    let mut next = sequence.clone();
    match op {
        EditOp::Insert { track, at, clip } => insert(&mut next, *track, *at, clip)?,
        EditOp::InsertAt { track, at, clip } => insert_at(&mut next, *track, *at, clip)?,
        EditOp::Overwrite { track, at, clip } => overwrite(&mut next, *track, *at, clip)?,
        EditOp::Move {
            clip,
            to_track,
            start,
        } => move_clip(&mut next, *clip, *to_track, *start)?,
        EditOp::MoveOverwrite {
            clip,
            to_track,
            start,
        } => move_overwrite(&mut next, *clip, *to_track, *start)?,
        EditOp::MoveInsert {
            clip,
            to_track,
            start,
        } => move_insert(&mut next, *clip, *to_track, *start)?,
        EditOp::TrimStart { clip, to } => trim_start(&mut next, *clip, *to)?,
        EditOp::TrimEnd { clip, to } => trim_end(&mut next, *clip, *to)?,
        EditOp::RippleTrimStart { clip, to } => ripple_trim_start(&mut next, *clip, *to)?,
        EditOp::RippleTrimEnd { clip, to } => ripple_trim_end(&mut next, *clip, *to)?,
        EditOp::Split { clip, at } => split(&mut next, *clip, *at)?,
        EditOp::Remove { clips } => remove(&mut next, clips)?,
        EditOp::RippleDelete { clips } => ripple_delete(&mut next, clips)?,
        EditOp::Slip { clip, by } => slip(&mut next, *clip, *by)?,
        EditOp::Slide { clip, by } => slide(&mut next, *clip, *by)?,
        EditOp::Roll { clip, by } => roll(&mut next, *clip, *by)?,
        EditOp::SetSpeed { clip, speed } => set_speed(&mut next, *clip, *speed)?,
        EditOp::SetRemap { clip, curve } => set_remap(&mut next, *clip, curve)?,
        EditOp::SetHold { clip, hold } => set_hold(&mut next, *clip, *hold)?,
        EditOp::SetColor { clip, color } => set_color(&mut next, *clip, color)?,
        EditOp::MoveKeyframe { clip, from, to } => move_keyframe(&mut next, *clip, *from, *to)?,
        EditOp::AddSubtitle { track, item } => add_subtitle(&mut next, *track, item)?,
        EditOp::SetSubtitle { id, item } => set_subtitle(&mut next, *id, item)?,
        EditOp::RemoveSubtitle { id } => remove_subtitle(&mut next, *id)?,
        EditOp::RemoveTrack { track } => remove_track(&mut next, *track)?,
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
///
/// 子模块（参数、字幕）也要用它，所以是 `pub(super)` 而不是私有。
pub(super) fn locate(sequence: &Sequence, clip: Id) -> Result<(usize, usize), EditError> {
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

/// 移动并用覆盖语义落位：落点区间里的内容让开。
fn move_overwrite(
    sequence: &mut Sequence,
    clip_id: Id,
    to_track: Id,
    start: Tick,
) -> Result<(), EditError> {
    let (mut clip, target) = take_for_move(sequence, clip_id, to_track, start)?;
    clear_range(sequence, target, start, start + clip.duration());
    clip.start = start;
    let track = &mut sequence.tracks[target];
    track.clips.push(clip);
    track.sort_clips();
    Ok(())
}

/// 移动并用插入语义落位：在落点切开后续片段并整体右移。
fn move_insert(
    sequence: &mut Sequence,
    clip_id: Id,
    to_track: Id,
    start: Tick,
) -> Result<(), EditError> {
    let (mut clip, target) = take_for_move(sequence, clip_id, to_track, start)?;
    shift_open(sequence, target, start, clip.duration());
    clip.start = start;
    let track = &mut sequence.tracks[target];
    track.clips.push(clip);
    track.sort_clips();
    Ok(())
}

/// 移动类操作的公共前半段：校验、从原轨道摘下来、确认目标轨道的类型与锁定。
///
/// 必须先摘再腾位置：片段自己也算轨道上的内容，不摘的话「覆盖」会把自己切一刀。
fn take_for_move(
    sequence: &mut Sequence,
    clip_id: Id,
    to_track: Id,
    start: Tick,
) -> Result<(ClipItem, usize), EditError> {
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
    let clip = sequence.tracks[from_track].clips.remove(from_index);
    Ok((clip, target))
}

/// 挪一组关键帧。同一时刻的标记是所有属性共用的，所以挪的是整组，不是单条曲线。
///
/// `to` 夹进片段内部而不是报错：拖着标记在片段上换位置时越过两端是常态，
/// 弹一句错误只会打断手感。`from` 处没有点时序列原样返回，界面据此不落撤销。
fn move_keyframe(
    sequence: &mut Sequence,
    clip_id: Id,
    from: Tick,
    to: Tick,
) -> Result<(), EditError> {
    let (track_index, clip_index) = locate(sequence, clip_id)?;
    let item = &mut sequence.tracks[track_index].clips[clip_index];
    let to = to.max(Tick::ZERO).min(item.duration());
    item.keys.move_marks(from, to);
    Ok(())
}

/// 删掉整条轨道。片段与转场都挂在轨道上，一起走，不留悬空引用。
///
/// 允许删到一条轨道都不剩：序列空着是合法状态，界面有「这条序列还没有轨道」的提示，
/// 想再导素材时加一条轨道就行。
fn remove_track(sequence: &mut Sequence, track_id: Id) -> Result<(), EditError> {
    let index = sequence
        .track_index(track_id)
        .ok_or(EditError::UnknownTrack(track_id))?;
    if sequence.tracks[index].locked {
        return Err(EditError::TrackLocked(track_id));
    }
    sequence.tracks.remove(index);
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
