//! 删除：普通删除留空隙，波纹删除把同轨后面的内容补齐。

use super::{EditError, Id, Sequence, Tick, checked_ids};
pub(super) fn remove(sequence: &mut Sequence, clips: &[Id]) -> Result<(), EditError> {
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

pub(super) fn ripple_delete(sequence: &mut Sequence, clips: &[Id]) -> Result<(), EditError> {
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
