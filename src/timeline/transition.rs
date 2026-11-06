//! 转场编辑：加 / 改 / 去掉，以及「这段能不能加、最多能加多长」的判断。
//!
//! 转场不改变序列总时长：它从「前一段的尾巴」和「后一段往前多取的素材」里借出
//! 一段重叠（见 `model::Transition`）。所以能不能加、能加多长，全看两侧余量：
//! 前一段得够长（`xfade` 的 `offset` 不能为负），后一段的 `source_in` 要够大。
//! 导出侧余量不足时会退化成硬切（见 `timeline::export::plan`），界面这层必须
//! 提前把话说清楚，否则用户以为加上了，成片里却是一条硬切。

use super::{Id, Sequence, Tick, Transition, TransitionKind};

/// 转场最短时长。再短看不出过渡，只是一次闪帧。
pub const MIN_TRANSITION: Tick = Tick::from_micros(100_000);

/// 转场最长时长。两秒的溶解已经很长，再长就不如换个剪法。
pub const MAX_TRANSITION: Tick = Tick::from_micros(2_000_000);

/// 第一次加转场用的时长：与旧版右键菜单里的固定值一致。
pub const DEFAULT_TRANSITION: Tick = Tick::from_micros(500_000);

/// 设置转场的结果。
pub struct TransitionApplied {
    pub sequence: Sequence,
    /// 实际生效的时长。超出余量时会比请求的短。
    pub duration: Tick,
    /// 请求的时长超出可用余量，已被钳到 `duration`。
    pub clamped: bool,
}

/// 转场加不上的原因。文案由这里给，界面只负责显示。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransitionError {
    UnknownClip(Id),
    /// 左边没有紧挨着的片段（中间有空档，或者它就是第一段）。
    NotAdjacent,
    /// 两侧余量不到 [`MIN_TRANSITION`]：加上去也只能退化成硬切。
    NoRoom(Tick),
    TrackLocked,
}

impl std::fmt::Display for TransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransitionError::UnknownClip(id) => write!(f, "片段 {id} 不存在"),
            TransitionError::NotAdjacent => {
                write!(f, "左边没有紧挨着的片段，加不了转场（先把两段接上）")
            }
            TransitionError::NoRoom(room) => write!(
                f,
                "两侧余量只有 {:.2} 秒，加不了转场：成片里会退化成硬切。把前一段留长一点，\
                 或者把后一段的素材入点往后挪",
                room.secs()
            ),
            TransitionError::TrackLocked => write!(f, "这条轨道已锁定，先解锁再改转场"),
        }
    }
}

impl std::error::Error for TransitionError {}

/// 后段片段能借到的余量。`None` 表示左边没有紧挨着的片段。
///
/// 取两侧的较小值：前一段不能比转场还短（`xfade` 的 `offset` 要非负），后一段
/// 往前多取的素材也不能早于素材的 0 点。
pub fn transition_room(sequence: &Sequence, after: Id) -> Option<Tick> {
    let track = sequence
        .tracks
        .iter()
        .find(|track| track.clip(after).is_some())?;
    let index = track.clip_index(after)?;
    let before = track.clips.get(index.checked_sub(1)?)?;
    let clip = &track.clips[index];
    (before.end() == clip.start).then(|| before.duration().min(clip.source_in))
}

/// 后段片段上现有的转场（没有就返回 `None`）。
pub fn current(sequence: &Sequence, after: Id) -> Option<Transition> {
    let track = sequence
        .tracks
        .iter()
        .find(|track| track.clip(after).is_some())?;
    let index = track.clip_index(after)?;
    let before = track.clips.get(index.checked_sub(1)?)?.id;
    track
        .transitions
        .iter()
        .find(|transition| transition.before == before && transition.after == after)
        .cloned()
}

/// 在两段之间设置转场：没有就新建，有就改类型与时长。
///
/// 时长会被钳到 `MIN_TRANSITION..=min(room, MAX_TRANSITION)`：用户可以随便点，
/// 但落进工程里的值必须是导出能兑现的那个。
pub fn set(
    sequence: &Sequence,
    after: Id,
    kind: TransitionKind,
    duration: Tick,
) -> Result<TransitionApplied, TransitionError> {
    let track = sequence
        .tracks
        .iter()
        .find(|track| track.clip(after).is_some())
        .ok_or(TransitionError::UnknownClip(after))?;
    if track.locked {
        return Err(TransitionError::TrackLocked);
    }
    let index = track
        .clip_index(after)
        .ok_or(TransitionError::UnknownClip(after))?;
    let before = track
        .clips
        .get(index.checked_sub(1).ok_or(TransitionError::NotAdjacent)?)
        .ok_or(TransitionError::NotAdjacent)?;
    if before.end() != track.clips[index].start {
        return Err(TransitionError::NotAdjacent);
    }

    let room = before.duration().min(track.clips[index].source_in);
    if room < MIN_TRANSITION {
        return Err(TransitionError::NoRoom(room));
    }
    let limit = room.min(MAX_TRANSITION);
    let wanted = duration.clamp(MIN_TRANSITION, limit);
    let clamped = wanted != duration;

    let mut next = sequence.clone();
    let id = next.alloc_id();
    let target = next
        .track_mut(track.id)
        .ok_or(TransitionError::UnknownClip(after))?;
    match target
        .transitions
        .iter_mut()
        .find(|transition| transition.before == before.id && transition.after == after)
    {
        Some(existing) => {
            existing.kind = kind;
            existing.duration = wanted;
        }
        None => target
            .transitions
            .push(Transition::new(id, before.id, after, kind, wanted)),
    }
    Ok(TransitionApplied {
        sequence: next,
        duration: wanted,
        clamped,
    })
}

/// 去掉两段之间的转场。本来就没有时原样返回，不算错误。
pub fn remove(sequence: &Sequence, after: Id) -> Result<Sequence, TransitionError> {
    let track = sequence
        .tracks
        .iter()
        .find(|track| track.clip(after).is_some())
        .ok_or(TransitionError::UnknownClip(after))?;
    if track.locked {
        return Err(TransitionError::TrackLocked);
    }
    let index = track
        .clip_index(after)
        .ok_or(TransitionError::UnknownClip(after))?;
    let before = track
        .clips
        .get(index.checked_sub(1).ok_or(TransitionError::NotAdjacent)?)
        .ok_or(TransitionError::NotAdjacent)?;
    let (track_id, before_id) = (track.id, before.id);

    let mut next = sequence.clone();
    if let Some(target) = next.track_mut(track_id) {
        target
            .transitions
            .retain(|transition| !(transition.before == before_id && transition.after == after));
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{AssetId, ClipItem, TrackKind};

    /// 两段紧挨着的片段：前一段 `before_len` 长，后一段的素材入点是 `source_in`。
    fn seq(source_in: Tick, before_len: Tick) -> (Sequence, Id, Id) {
        let mut sequence = Sequence::default();
        let track = sequence.add_track(TrackKind::Video);
        let first = sequence.alloc_id();
        let second = sequence.alloc_id();
        let track = sequence.track_mut(track).expect("轨道刚加进去");
        track.clips.push(ClipItem::new(
            first,
            AssetId::new(1),
            Tick::ZERO,
            before_len,
            Tick::ZERO,
        ));
        track.clips.push(ClipItem::new(
            second,
            AssetId::new(1),
            source_in,
            source_in + Tick::from_secs(2.0),
            before_len,
        ));
        (sequence, first, second)
    }

    #[test]
    fn 余量取两侧较小值() {
        let (sequence, _, second) = seq(Tick::from_secs(1.0), Tick::from_secs(2.0));
        assert_eq!(
            transition_room(&sequence, second),
            Some(Tick::from_secs(1.0)),
            "后段余量更小"
        );

        let (sequence, _, second) = seq(Tick::from_secs(5.0), Tick::from_secs(2.0));
        assert_eq!(
            transition_room(&sequence, second),
            Some(Tick::from_secs(2.0)),
            "前段更短"
        );
    }

    #[test]
    fn 有空档就不是接缝() {
        let (mut sequence, _, second) = seq(Tick::from_secs(1.0), Tick::from_secs(2.0));
        sequence.tracks[0].clips[1].start = Tick::from_secs(3.0);
        assert_eq!(transition_room(&sequence, second), None);
        assert_eq!(
            set(&sequence, second, TransitionKind::Fade, DEFAULT_TRANSITION).err(),
            Some(TransitionError::NotAdjacent)
        );
    }

    #[test]
    fn 超出余量的时长被钳住() {
        let (sequence, _, second) = seq(Tick::from_millis(300), Tick::from_secs(2.0));
        let applied = set(
            &sequence,
            second,
            TransitionKind::CrossDissolve,
            Tick::from_secs(2.0),
        )
        .expect("余量够 0.3 秒");
        assert_eq!(applied.duration, Tick::from_millis(300));
        assert!(applied.clamped);
        assert_eq!(
            applied.sequence.tracks[0].transitions[0].duration,
            Tick::from_millis(300)
        );
    }

    #[test]
    fn 再设一次是改类型而不是加一条() {
        let (sequence, _, second) = seq(Tick::from_secs(1.0), Tick::from_secs(2.0));
        let first = set(
            &sequence,
            second,
            TransitionKind::CrossDissolve,
            DEFAULT_TRANSITION,
        )
        .expect("能加");
        let again = set(
            &first.sequence,
            second,
            TransitionKind::Wipe,
            Tick::from_millis(800),
        )
        .expect("能改");
        assert_eq!(again.sequence.tracks[0].transitions.len(), 1);
        let transition = &again.sequence.tracks[0].transitions[0];
        assert_eq!(transition.kind, TransitionKind::Wipe);
        assert_eq!(transition.duration, Tick::from_millis(800));
        assert_eq!(
            current(&again.sequence, second).map(|t| t.kind),
            Some(TransitionKind::Wipe)
        );
    }

    #[test]
    fn 余量不够最短时长就拒绝() {
        let (sequence, _, second) = seq(Tick::from_millis(50), Tick::from_secs(2.0));
        assert_eq!(
            set(&sequence, second, TransitionKind::Fade, DEFAULT_TRANSITION).err(),
            Some(TransitionError::NoRoom(Tick::from_millis(50)))
        );
    }

    #[test]
    fn 去掉转场只动这一处接缝() {
        let (sequence, _, second) = seq(Tick::from_secs(1.0), Tick::from_secs(2.0));
        let applied =
            set(&sequence, second, TransitionKind::Slide, DEFAULT_TRANSITION).expect("能加");
        let removed = remove(&applied.sequence, second).expect("能去");
        assert!(removed.tracks[0].transitions.is_empty());
        // 本来就没有：再删一次不算错，序列原样。
        let again = remove(&removed, second).expect("删空不算错");
        assert!(again.tracks[0].transitions.is_empty());
    }

    #[test]
    fn 锁定的轨道不给改() {
        let (mut sequence, _, second) = seq(Tick::from_secs(1.0), Tick::from_secs(2.0));
        sequence.tracks[0].locked = true;
        assert_eq!(
            set(&sequence, second, TransitionKind::Fade, DEFAULT_TRANSITION).err(),
            Some(TransitionError::TrackLocked)
        );
        assert_eq!(
            remove(&sequence, second).err(),
            Some(TransitionError::TrackLocked)
        );
    }
}
