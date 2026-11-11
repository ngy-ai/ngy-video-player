//! 字幕条目的增删改：轨道必须是未锁定的字幕轨，同轨条目不许重叠。
//!
//! 字幕不是 `ClipItem`：它没有素材、没有倍率，也不参与拖放与切分，所以校验与
//! 片段那套分开写。负起点一律拒绝——那是数据错误，不该被 `normalize` 悄悄夹成 0；
//! 其余坏值（空区间、空文本、字号越界）规整后落盘。

use super::{EditError, Id, Sequence, SubtitleItem};
use crate::timeline::TrackKind;

/// 字幕编辑的轨道定位：轨道存在、是字幕轨、没锁，三件事一次查完。
fn subtitle_lane(sequence: &Sequence, track: Id) -> Result<usize, EditError> {
    let index = sequence
        .track_index(track)
        .ok_or(EditError::UnknownTrack(track))?;
    let lane = &sequence.tracks[index];
    if lane.locked {
        return Err(EditError::TrackLocked(track));
    }
    if lane.kind != TrackKind::Subtitle {
        return Err(EditError::WrongTrackKind);
    }
    Ok(index)
}

/// 字幕的时间区间要合法，且同一轨上不与别的条目重叠（首尾相接不算重叠）。
fn check_subtitle(
    sequence: &Sequence,
    index: usize,
    item: &SubtitleItem,
    except: Option<Id>,
) -> Result<(), EditError> {
    if item.start.is_negative() {
        return Err(EditError::NegativeTime);
    }
    if item.end <= item.start {
        return Err(EditError::EmptyClip);
    }
    let lane = &sequence.tracks[index];
    let overlap = lane
        .subtitles
        .iter()
        .any(|other| Some(other.id) != except && other.start < item.end && item.start < other.end);
    if overlap {
        return Err(EditError::Overlap(lane.id));
    }
    Ok(())
}

pub(super) fn add_subtitle(
    sequence: &mut Sequence,
    track: Id,
    item: &SubtitleItem,
) -> Result<(), EditError> {
    let index = subtitle_lane(sequence, track)?;
    if sequence.id_is_used(item.id) {
        return Err(EditError::DuplicateId(item.id));
    }
    // 负起点是数据错误，不该被 `normalize` 悄悄夹成 0；其余坏值（空区间、
    // 空文本、字号越界）规整后落盘。
    if item.start.is_negative() {
        return Err(EditError::NegativeTime);
    }
    let mut item = item.clone();
    item.normalize();
    check_subtitle(sequence, index, &item, None)?;
    sequence.tracks[index].subtitles.push(item);
    sequence.tracks[index].sort_subtitles();
    // 直接给 id 的调用方（导入、测试）未必动过计数器。
    sequence.repair_next_id();
    Ok(())
}

pub(super) fn set_subtitle(
    sequence: &mut Sequence,
    id: Id,
    item: &SubtitleItem,
) -> Result<(), EditError> {
    let track = sequence
        .subtitle_track(id)
        .ok_or(EditError::UnknownSubtitle(id))?;
    let index = subtitle_lane(sequence, track)?;
    if item.start.is_negative() {
        return Err(EditError::NegativeTime);
    }
    let mut item = item.clone();
    item.id = id;
    item.normalize();
    check_subtitle(sequence, index, &item, Some(id))?;
    let lane = &mut sequence.tracks[index];
    let position = lane
        .subtitles
        .iter()
        .position(|entry| entry.id == id)
        .ok_or(EditError::UnknownSubtitle(id))?;
    lane.subtitles[position] = item;
    lane.sort_subtitles();
    Ok(())
}

pub(super) fn remove_subtitle(sequence: &mut Sequence, id: Id) -> Result<(), EditError> {
    let track = sequence
        .subtitle_track(id)
        .ok_or(EditError::UnknownSubtitle(id))?;
    let index = subtitle_lane(sequence, track)?;
    sequence.tracks[index]
        .subtitles
        .retain(|entry| entry.id != id);
    Ok(())
}
