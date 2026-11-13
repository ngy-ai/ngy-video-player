//! 变速与定格：改倍率、加片尾定格。
//!
//! 两者都只改片段**在时间线上的长度**，不动素材跨度。长度一变，同轨后面的片段
//! 就整体让位（往前缩或往后推），保持原来的间隔——这是波纹编辑，和插入 / 波纹删除
//! 是同一套手感：改一段的时长，后面的跟着走，不会压在一起。
//!
//! 不变量「同轨不重叠」在这里由 ripple 保证：不从后面推的话，变慢的那一刻就重叠了。

use crate::timeline::{ClipItem, Id, Sequence, SpeedCurve, Tick};

/// 设置倍率。返回是否真的改了（片段不存在或倍率没变时为 `false`）。
pub fn set_speed(sequence: &mut Sequence, clip: Id, speed: f32) -> bool {
    let speed = if speed.is_finite() {
        speed.clamp(ClipItem::MIN_SPEED, ClipItem::MAX_SPEED)
    } else {
        return false;
    };
    let Some(track) = sequence.clip_track(clip) else {
        return false;
    };
    let Some(before) = sequence.clip(clip).map(ClipItem::duration) else {
        return false;
    };
    let Some(item) = sequence.clip_mut(clip) else {
        return false;
    };
    if (item.speed - speed).abs() < 1e-6 {
        return false;
    }
    item.speed = speed;
    let after = item.duration();
    ripple(sequence, track, clip, after - before);
    true
}

/// 设置片尾定格。`Tick::ZERO` 表示取消。
pub fn set_hold(sequence: &mut Sequence, clip: Id, hold: Tick) -> bool {
    let hold = hold.max(Tick::ZERO);
    let Some(track) = sequence.clip_track(clip) else {
        return false;
    };
    let Some(before) = sequence.clip(clip).map(ClipItem::duration) else {
        return false;
    };
    let Some(item) = sequence.clip_mut(clip) else {
        return false;
    };
    if item.hold == hold {
        return false;
    }
    item.hold = hold;
    let after = item.duration();
    ripple(sequence, track, clip, after - before);
    true
}

/// 换上一条速度曲线（时间重映射）。空曲线表示回到常量倍率 `speed`。
///
/// 时长会跟着曲线积分变，所以和 [`set_speed`] 一样要做波纹让位。曲线先规整一遍：
/// 位置不为负、倍率夹进量程、同位置只留一个——界面拖出来的值带浮点毛刺，
/// 存进工程前统一。
pub fn set_remap(sequence: &mut Sequence, clip: Id, curve: &SpeedCurve) -> bool {
    let mut clean = SpeedCurve::default();
    for point in &curve.points {
        clean.insert(point.at, point.rate);
    }
    let Some(track) = sequence.clip_track(clip) else {
        return false;
    };
    let Some(before) = sequence.clip(clip).map(ClipItem::duration) else {
        return false;
    };
    let Some(item) = sequence.clip_mut(clip) else {
        return false;
    };
    if item.remap == clean {
        return false;
    }
    item.remap = clean;
    let after = item.duration();
    ripple(sequence, track, clip, after - before);
    true
}

/// 把 `clip` 之后（按起点）的片段整体挪 `delta`，避免变速后压到一起。
fn ripple(sequence: &mut Sequence, track: Id, clip: Id, delta: Tick) {
    if delta == Tick::ZERO {
        return;
    }
    let Some(lane) = sequence.track_mut(track) else {
        return;
    };
    let Some(index) = lane.clip_index(clip) else {
        return;
    };
    let start = lane.clips[index].start;
    // 先取终点：循环里借用的是 `clips` 的可变引用，不能再回头读它。
    let floor = lane.clips[index].end();
    for item in lane.clips.iter_mut().skip(index + 1) {
        if item.start >= start {
            // 往前缩时不能把后面的片段推到当前片段里面去。
            item.start = (item.start + delta).max(floor);
        }
    }
    lane.sort_clips();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{AssetId, Rational, TrackKind};

    fn lane(items: &[(i64, i64)]) -> (Sequence, Vec<Id>) {
        let mut sequence = Sequence::new("测试", Rational::FPS_25, 1280, 720);
        let track = sequence.tracks[0].id;
        let mut ids = Vec::new();
        let mut cursor = Tick::ZERO;
        for (index, (seconds, start)) in items.iter().enumerate() {
            let id = sequence.alloc_id();
            let source_in = Tick::from_micros(index as i64 * 10_000_000);
            let clip = ClipItem::new(
                id,
                AssetId::new(1),
                source_in,
                source_in + Tick::from_secs(*seconds as f64),
                Tick::from_secs(*start as f64),
            );
            cursor = clip.end();
            sequence.track_mut(track).unwrap().clips.push(clip);
            ids.push(id);
        }
        let _ = cursor;
        (sequence, ids)
    }

    #[test]
    fn 变快让后面的片段整体前移() {
        let (mut sequence, ids) = lane(&[(2, 0), (2, 2)]);
        assert!(set_speed(&mut sequence, ids[0], 2.0));
        let track = &sequence.tracks[0];
        assert_eq!(track.clips[0].duration(), Tick::from_secs(1.0));
        assert_eq!(track.clips[1].start, Tick::from_secs(1.0), "后一段跟着前移");
    }

    #[test]
    fn 变慢把后面的片段推开且不重叠() {
        let (mut sequence, ids) = lane(&[(2, 0), (2, 2)]);
        assert!(set_speed(&mut sequence, ids[0], 0.5));
        let track = &sequence.tracks[0];
        assert_eq!(track.clips[0].duration(), Tick::from_secs(4.0));
        assert_eq!(track.clips[1].start, Tick::from_secs(4.0));
        assert!(track.clips[0].end() <= track.clips[1].start);
    }

    #[test]
    fn 定格同样往后推() {
        let (mut sequence, ids) = lane(&[(2, 0), (2, 2)]);
        assert!(set_hold(&mut sequence, ids[0], Tick::from_secs(3.0)));
        assert_eq!(sequence.tracks[0].clips[0].duration(), Tick::from_secs(5.0));
        assert_eq!(sequence.tracks[0].clips[1].start, Tick::from_secs(5.0));
    }

    #[test]
    fn 取消定格回到原来的长度() {
        let (mut sequence, ids) = lane(&[(2, 0)]);
        set_hold(&mut sequence, ids[0], Tick::from_secs(3.0));
        assert!(set_hold(&mut sequence, ids[0], Tick::ZERO));
        assert_eq!(sequence.tracks[0].clips[0].duration(), Tick::from_secs(2.0));
    }

    #[test]
    fn 倍率被夹到量程且重复设置不算改动() {
        let (mut sequence, ids) = lane(&[(2, 0)]);
        set_speed(&mut sequence, ids[0], 100.0);
        assert_eq!(
            sequence.clips().next().unwrap().1.speed,
            ClipItem::MAX_SPEED
        );
        assert!(!set_speed(&mut sequence, ids[0], 100.0), "值没变");
        assert!(!set_speed(&mut sequence, Id::new(999), 2.0), "片段不存在");
    }

    #[test]
    fn 锁轨之外的行为不受轨道类型影响() {
        // 音频轨上的片段也能变速（导出侧用 atempo 对齐）。
        let mut sequence = Sequence::new("测试", Rational::FPS_25, 1280, 720);
        sequence.add_track(TrackKind::Audio);
        let track = sequence.tracks[1].id;
        let id = sequence.alloc_id();
        sequence.track_mut(track).unwrap().clips.push(ClipItem::new(
            id,
            AssetId::new(1),
            Tick::ZERO,
            Tick::from_secs(4.0),
            Tick::ZERO,
        ));
        assert!(set_speed(&mut sequence, id, 2.0));
        assert_eq!(
            sequence.clips().next().unwrap().1.end(),
            Tick::from_secs(2.0)
        );
    }
}
