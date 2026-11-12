//! 轨道目标：素材的音 / 视频分别落到哪条轨上。
//!
//! 用户在轨道头上点一下「目标」，之后三点 / 四点编辑就往那条轨上放；没指定时退回
//! 「同类里第一条没锁的」。两条规则合成一个纯函数，是为了让「有目标也不会落到错误
//! 类型」这件事能被单测钉住，而不是散在界面里靠运气。

use super::{Id, Sequence, TrackKind};

/// 解析素材的落点轨道。
///
/// 顺序：首选轨（存在、类型对、没锁）→ 同类里第一条没锁的 → `None`（没有可用的轨）。
/// 首选轨被锁或类型不符时**不报错**，安静地退回第二位：用户把锁解开后目标还在，
/// 没必要为此中断一次编辑。
pub fn resolve_target(sequence: &Sequence, kind: TrackKind, preferred: Option<Id>) -> Option<Id> {
    if let Some(id) = preferred
        && let Some(track) = sequence.track(id)
        && track.kind == kind
        && !track.locked
    {
        return Some(id);
    }
    sequence
        .tracks
        .iter()
        .find(|track| track.kind == kind && !track.locked)
        .map(|track| track.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::Sequence;
    use crate::timeline::time::Tick;
    use crate::timeline::{AssetId, ClipItem};

    fn sequence() -> Sequence {
        Sequence::new("测试", crate::timeline::Rational::FPS_25, 1920, 1080)
    }

    #[test]
    fn 没指定目标时取第一条没锁的同类轨() {
        let mut sequence = sequence();
        let video = sequence.tracks[0].id;
        let audio = sequence.tracks[1].id;
        assert_eq!(
            resolve_target(&sequence, TrackKind::Video, None),
            Some(video)
        );
        assert_eq!(
            resolve_target(&sequence, TrackKind::Audio, None),
            Some(audio)
        );

        // 第一条锁了就给下一条。
        let second_video = sequence.add_track(TrackKind::Video);
        sequence.track_mut(video).unwrap().locked = true;
        assert_eq!(
            resolve_target(&sequence, TrackKind::Video, None),
            Some(second_video)
        );
    }

    #[test]
    fn 目标轨类型不符或已锁时退回同类第一条() {
        let mut sequence = sequence();
        let video = sequence.tracks[0].id;
        let audio = sequence.tracks[1].id;

        // 拿音频轨当视频素材的目标：类型不符，退回视频轨。
        assert_eq!(
            resolve_target(&sequence, TrackKind::Video, Some(audio)),
            Some(video)
        );

        // 目标轨锁上了：退回同类里第一条没锁的（这里就是它自己之外的那条）。
        let second_video = sequence.add_track(TrackKind::Video);
        sequence.track_mut(second_video).unwrap().locked = true;
        assert_eq!(
            resolve_target(&sequence, TrackKind::Video, Some(second_video)),
            Some(video)
        );

        // 指定的轨类型对且没锁：就用它。
        sequence.track_mut(second_video).unwrap().locked = false;
        assert_eq!(
            resolve_target(&sequence, TrackKind::Video, Some(second_video)),
            Some(second_video)
        );
    }

    #[test]
    fn 同类轨全被锁就没有落点() {
        let mut sequence = sequence();
        sequence.track_mut(sequence.tracks[0].id).unwrap().locked = true;
        assert_eq!(resolve_target(&sequence, TrackKind::Video, None), None);
        // 空序列里连轨都没有。
        let empty = Sequence::new("空", crate::timeline::Rational::FPS_25, 1920, 1080);
        let empty = {
            let mut empty = empty;
            empty.tracks.clear();
            empty
        };
        assert_eq!(resolve_target(&empty, TrackKind::Audio, None), None);
    }

    #[test]
    fn 目标轨上的片段仍按原样算时长() {
        // 顺手钉一下测试用到的时间单位：落点判断不掺秒。
        let mut sequence = sequence();
        let track = sequence.tracks[0].id;
        let id = sequence.alloc_id();
        sequence.track_mut(track).unwrap().clips.push(ClipItem::new(
            id,
            AssetId::new(1),
            Tick::ZERO,
            Tick::from_secs(2.0),
            Tick::ZERO,
        ));
        assert_eq!(sequence.track(track).unwrap().end(), Tick::from_secs(2.0));
    }
}
