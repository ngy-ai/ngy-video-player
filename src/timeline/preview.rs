//! 预览取帧：序列时间 → 该看哪一段素材的哪一时刻。
//!
//! 规则必须和导出一致（M4-T03）：只有**未静音 / 未隐藏**的视频轨参与，轨道顺序
//! 就是叠放顺序（列表靠上的画在上面）。导出那边用同一份 [`enabled_video_tracks`]
//! 选轨，两边的「哪些轨参与输出」因此不会各写一遍、慢慢长歪。
//!
//! 预览一次只解一路画面，所以取**最上面那条盖住该时刻的轨**；真正的多路叠加只有
//! 导出（`overlay`）做得到。

use super::{AssetId, Id, Sequence, Tick, Track, TrackKind};

/// 序列某一时刻该显示的画面。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewFrame {
    /// 该显示哪段素材。
    pub asset: AssetId,
    /// 素材内部的时刻，可以直接拿去 seek。
    pub source: Tick,
    /// 这段画面来自哪条轨（调试与界面提示用）。
    pub track: Id,
}

/// 参与输出的视频轨，按叠放顺序（先上后下）。静音 / 隐藏的、空轨都跳过。
pub fn enabled_video_tracks(sequence: &Sequence) -> impl Iterator<Item = &Track> {
    sequence
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video && !track.muted && !track.clips.is_empty())
}

/// 播放头在 `at` 时该显示哪一帧。`None` 表示这一刻没有任何画面（空档）。
pub fn resolve(sequence: &Sequence, at: Tick) -> Option<PreviewFrame> {
    enabled_video_tracks(sequence).find_map(|track| {
        track
            .clips
            .iter()
            .find(|clip| clip.covers(at))
            .map(|clip| PreviewFrame {
                asset: clip.asset_id,
                source: clip.source_in + (at - clip.start),
                track: track.id,
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{ClipItem, Rational};

    fn s(seconds: f64) -> Tick {
        Tick::from_secs(seconds)
    }

    fn sequence() -> Sequence {
        Sequence::new("测试", Rational::FPS_25, 1920, 1080)
    }

    /// 往 `track` 上放一个片段：素材内 `[in, out)`，时间线从 `start` 开始。
    fn put(
        sequence: &mut Sequence,
        track: Id,
        asset: u64,
        source_in: f64,
        source_out: f64,
        start: f64,
    ) {
        let id = sequence.alloc_id();
        sequence
            .track_mut(track)
            .expect("轨道存在")
            .clips
            .push(ClipItem::new(
                id,
                AssetId::new(asset),
                s(source_in),
                s(source_out),
                s(start),
            ));
    }

    #[test]
    fn 序列时间换算成素材时间() {
        let mut sequence = sequence();
        let video = sequence.tracks[0].id;
        put(&mut sequence, video, 7, 10.0, 12.0, 3.0);

        let frame = resolve(&sequence, s(4.5)).expect("这一刻有画面");
        assert_eq!(frame.asset, AssetId::new(7));
        assert_eq!(frame.source, s(11.5), "素材内的时刻 = in + (at - start)");
    }

    #[test]
    fn 最上面那条轨盖住下层() {
        let mut sequence = sequence();
        let top = sequence.tracks[0].id;
        put(&mut sequence, top, 1, 0.0, 5.0, 0.0);
        let lower = sequence.add_track(TrackKind::Video);
        put(&mut sequence, lower, 2, 0.0, 5.0, 0.0);

        let frame = resolve(&sequence, s(1.0)).expect("有画面");
        assert_eq!(frame.track, top, "列表靠上的轨优先");
        assert_eq!(frame.asset, AssetId::new(1));
    }

    #[test]
    fn 关掉的轨不参与预览() {
        let mut sequence = sequence();
        let top = sequence.tracks[0].id;
        put(&mut sequence, top, 1, 0.0, 5.0, 0.0);
        let lower = sequence.add_track(TrackKind::Video);
        put(&mut sequence, lower, 2, 0.0, 5.0, 0.0);
        sequence.track_mut(top).unwrap().muted = true;

        assert_eq!(resolve(&sequence, s(1.0)).unwrap().asset, AssetId::new(2));
    }

    #[test]
    fn 空档没有画面() {
        let mut sequence = sequence();
        let video = sequence.tracks[0].id;
        put(&mut sequence, video, 1, 0.0, 1.0, 0.0);
        assert_eq!(
            resolve(&sequence, s(0.5)).map(|frame| frame.asset),
            Some(AssetId::new(1))
        );
        assert_eq!(resolve(&sequence, s(1.5)), None);
    }
}
