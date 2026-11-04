//! 预览取帧：序列时间 → 该看哪一段素材的哪一时刻。
//!
//! 规则必须和导出一致（M4-T03）：只有**未静音 / 未隐藏**的视频轨参与，轨道顺序
//! 就是叠放顺序（列表靠上的画在上面）。导出那边用同一份 [`enabled_video_tracks`]
//! 选轨，两边的「哪些轨参与输出」因此不会各写一遍、慢慢长歪。
//!
//! 预览一次只解一路画面，所以取**最上面那条盖住该时刻的轨**；真正的多路叠加只有
//! 导出（`overlay`）做得到。

use std::time::Duration;

use super::{AssetId, Id, Sequence, Tick, Track, TrackKind};

/// 预览播放一次最多往前跳过多久的真实时间。
///
/// 界面卡一下（切窗口、开对话框）之后不该让播放头一下子跳出好几秒——那会把中间
/// 的片段整段跳过去，看起来像「丢了一段」。
pub const MAX_STEP: Duration = Duration::from_millis(250);

/// 预览画面与时间线的偏差超过这个值才值得补一次 seek。
///
/// 每补一次 seek 声音就要重新起一次，所以阈值取得比「人眼能察觉」宽：加载耗时、
/// 缓冲这类上百毫秒的偏差要纠正，几十毫秒的抖动不值得。
pub const MAX_DRIFT: Duration = Duration::from_millis(350);

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

/// 预览播放停在哪儿：有工作区就用出点，否则到序列总长。
///
/// 工作区出点超过序列长度时按序列长度算：出点能设到序列外，但播放不能走到
/// 「什么都没有」的那一段去。
pub fn stop_at(duration: Tick, range: Option<(Tick, Tick)>) -> Tick {
    match range {
        Some((_, end)) => end.min(duration),
        None => duration,
    }
}

/// 按下播放时从哪儿开始。
///
/// 播放头已经停在终点（或越过终点）时从头再来：按播放没反应是最让人困惑的
/// 一种状态。有工作区就从入点起——反复看一小段时不必每次手动回拖。
pub fn start_at(playhead: Tick, stop: Tick, range: Option<(Tick, Tick)>) -> Tick {
    if playhead < stop {
        return playhead;
    }
    range.map(|(start, _)| start).unwrap_or(Tick::ZERO)
}

/// 播放头往前走 `elapsed` 真实时间（乘倍速），到 `stop` 停住。
///
/// 走的是墙钟而不是 mpv 报的时间：空档里 mpv 是停着的，用它的位置播放头会卡住。
/// 单步上限见 [`MAX_STEP`]；倍速取自 mpv 的 `speed`，两边必须乘同一个数，
/// 否则倍速一改音画和播放头就会越离越远。
pub fn advance(playhead: Tick, elapsed: Duration, speed: f64, stop: Tick) -> Tick {
    let elapsed = elapsed.min(MAX_STEP).as_secs_f64();
    // 倍速异常（NaN / 0 / 离谱的大数）时按原速走：宁可慢一拍，也别让播放头飞跃。
    let speed = if speed.is_finite() && speed > 0.0 {
        speed.clamp(0.1, 4.0)
    } else {
        1.0
    };
    (playhead + Tick::from_secs(elapsed * speed)).min(stop)
}

/// 预览画面与时间线的偏差是否大到值得补一次 seek。
///
/// `source` 是播放头对应的素材内时刻，`position` 是 mpv 报的当前时刻。位置不是
/// 有限数（还没加载出来）时不纠正：那时 seek 只会把加载搅乱。
pub fn out_of_sync(source: Tick, position: f64) -> bool {
    position.is_finite() && (position - source.secs()).abs() > MAX_DRIFT.as_secs_f64()
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

    // -- 播放时钟 ----------------------------------------------------------

    #[test]
    fn 播放头按真实时间与倍速推进() {
        let stop = s(60.0);
        let at = advance(s(2.0), Duration::from_millis(100), 1.0, stop);
        assert_eq!(at, s(2.1), "原速：走多久就是多久");

        let fast = advance(s(2.0), Duration::from_millis(100), 2.0, stop);
        assert_eq!(fast, s(2.2), "两倍速：时钟也要走两步");
    }

    #[test]
    fn 推进到终点就停住() {
        let stop = s(5.0);
        assert_eq!(advance(s(4.9), Duration::from_millis(200), 1.0, stop), stop);
        assert_eq!(
            advance(s(5.0), Duration::from_millis(200), 1.0, stop),
            stop,
            "已经在终点时保持不动"
        );
    }

    #[test]
    fn 界面卡顿不会跳过整段() {
        // 卡了两秒也只是走一个上限步长，不会把中间的片段一口气跳过去。
        let at = advance(s(1.0), Duration::from_secs(2), 1.0, s(60.0));
        assert_eq!(at, s(1.25), "上限是 MAX_STEP（250 毫秒）");
    }

    #[test]
    fn 倍速异常时按原速走() {
        let stop = s(60.0);
        for speed in [f64::NAN, 0.0, -1.0, f64::INFINITY] {
            assert_eq!(
                advance(s(1.0), Duration::from_millis(100), speed, stop),
                s(1.1),
                "{speed} 不是可用倍速"
            );
        }
    }

    #[test]
    fn 终点优先用工作区出点() {
        assert_eq!(stop_at(s(100.0), Some((s(10.0), s(20.0)))), s(20.0));
        assert_eq!(stop_at(s(100.0), None), s(100.0));
        assert_eq!(
            stop_at(s(30.0), Some((s(10.0), s(90.0)))),
            s(30.0),
            "出点越过序列长度时按序列长度收"
        );
    }

    #[test]
    fn 播完之后再按播放回到工作区入点() {
        let range = Some((s(10.0), s(20.0)));
        let stop = stop_at(s(100.0), range);
        assert_eq!(start_at(s(5.0), stop, range), s(5.0), "没到终点就原地开始");
        assert_eq!(start_at(stop, stop, range), s(10.0), "停在终点时回到入点");
        assert_eq!(
            start_at(s(25.0), stop, range),
            s(10.0),
            "越过终点（比如拖到了工作区外）同样从头来"
        );
        assert_eq!(start_at(s(100.0), s(100.0), None), Tick::ZERO);
    }

    #[test]
    fn 偏差小于阈值时不纠正() {
        assert!(!out_of_sync(s(10.0), 10.1), "一百毫秒内的抖动不用管");
        assert!(out_of_sync(s(10.0), 11.0), "差出一秒就该纠正");
        assert!(!out_of_sync(s(10.0), f64::NAN), "位置还没读出来时不要 seek");
    }
}
