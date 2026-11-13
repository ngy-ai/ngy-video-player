//! 选声规则的单测：取素材时刻、静音、独奏与音量。

use super::*;
use crate::timeline::mixer::GainPoint;
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
fn 音频轨片段按时刻取素材() {
    let mut sequence = sequence();
    let audio = sequence.tracks[1].id;
    put(&mut sequence, audio, 7, 1.0, 3.0, 2.0);

    assert_eq!(resolve_audio(&sequence, s(1.0)), None, "片段之前没有声音");
    let frame = resolve_audio(&sequence, s(2.5)).expect("片段之内有声");
    assert_eq!(frame.asset, AssetId::new(7));
    assert_eq!(frame.source, s(1.5), "素材 1 秒处的片段，2.5 秒对应 1.5 秒");
    assert_eq!(resolve_audio(&sequence, s(4.0)), None, "片段之后没有声音");
}

#[test]
fn 音频轨片段也认倍率() {
    let mut sequence = sequence();
    let audio = sequence.tracks[1].id;
    put(&mut sequence, audio, 7, 0.0, 4.0, 0.0);
    sequence.tracks[1].clips[0].speed = 2.0;

    assert_eq!(resolve_audio(&sequence, s(0.5)).unwrap().source, s(1.0));
    assert_eq!(resolve_audio(&sequence, s(2.0)), None, "2 倍速只占 2 秒");
}

#[test]
fn 静音的音频轨不出声() {
    let mut sequence = sequence();
    let audio = sequence.tracks[1].id;
    put(&mut sequence, audio, 7, 0.0, 2.0, 0.0);
    sequence.track_mut(audio).unwrap().muted = true;

    assert_eq!(resolve_audio(&sequence, s(0.5)), None);
}

#[test]
fn 多条音频轨取靠上的那条() {
    let mut sequence = sequence();
    let first = sequence.tracks[1].id;
    let second = sequence.add_track(TrackKind::Audio);
    put(&mut sequence, first, 1, 0.0, 2.0, 0.0);
    put(&mut sequence, second, 2, 0.0, 2.0, 0.0);

    assert_eq!(
        resolve_audio(&sequence, s(0.5)).unwrap().asset,
        AssetId::new(1),
        "只要一条时取列表靠上的那条"
    );
}

#[test]
fn 多条音频轨同时出声各带自己的音量() {
    let mut sequence = sequence();
    let first = sequence.tracks[1].id;
    let second = sequence.add_track(TrackKind::Audio);
    put(&mut sequence, first, 1, 0.0, 2.0, 0.0);
    put(&mut sequence, second, 2, 0.0, 2.0, 0.0);
    sequence.track_mut(first).unwrap().gain = 0.5;
    sequence.track_mut(second).unwrap().gain = 2.0;

    let layers = audio_layers(&sequence, s(0.5));
    assert_eq!(layers.len(), 2, "两条轨都在响");
    assert_eq!(layers[0].asset, AssetId::new(1));
    assert_eq!(layers[0].gain, 0.5);
    assert_eq!(layers[1].asset, AssetId::new(2));
    assert_eq!(layers[1].gain, 2.0);
}

#[test]
fn 预览增益走三层连乘并带出声像() {
    let mut sequence = sequence();
    let audio = sequence.tracks[1].id;
    put(&mut sequence, audio, 1, 0.0, 2.0, 0.0);
    let track = sequence.track_mut(audio).unwrap();
    track.gain = 0.5;
    track.pan = -0.5;
    track.automation = vec![GainPoint::new(s(0.0), 1.0), GainPoint::new(s(2.0), 0.5)];
    track.clips[0].gain = 0.8;

    // 第 1 秒：片段 0.8 × 自动化 0.75 × 推子 0.5 = 0.3。
    let layers = audio_layers(&sequence, s(1.0));
    assert_eq!(layers.len(), 1);
    assert!(
        (layers[0].gain - 0.3).abs() < 1e-4,
        "预览增益 {}",
        layers[0].gain
    );
    assert!(
        (layers[0].pan + 0.5).abs() < 1e-6,
        "预览也要拿到声像（导出侧同一个值）"
    );

    // 自动化起点处是 0.8 × 1.0 × 0.5 = 0.4：说明这条线真的在走，不是取了个常量。
    assert!((audio_layers(&sequence, s(0.0))[0].gain - 0.4).abs() < 1e-4);
}

#[test]
fn 独奏只留独奏的轨() {
    let mut sequence = sequence();
    let first = sequence.tracks[1].id;
    let second = sequence.add_track(TrackKind::Audio);
    put(&mut sequence, first, 1, 0.0, 2.0, 0.0);
    put(&mut sequence, second, 2, 0.0, 2.0, 0.0);
    sequence.track_mut(second).unwrap().solo = true;

    let layers = audio_layers(&sequence, s(0.5));
    assert_eq!(layers.len(), 1, "别的轨被独奏挤掉");
    assert_eq!(layers[0].asset, AssetId::new(2));

    // 独奏优先于静音：既静音又独奏的轨仍然是响的。
    sequence.track_mut(second).unwrap().muted = true;
    assert_eq!(audio_layers(&sequence, s(0.5)).len(), 1);

    // 关掉独奏之后回到「只看 muted」：那条静音了，另一条恢复。
    sequence.track_mut(second).unwrap().solo = false;
    let layers = audio_layers(&sequence, s(0.5));
    assert_eq!(layers.len(), 1);
    assert_eq!(layers[0].asset, AssetId::new(1));
}

#[test]
fn 音量坏值按原样且夹到上限() {
    let mut sequence = sequence();
    let audio = sequence.tracks[1].id;
    put(&mut sequence, audio, 1, 0.0, 2.0, 0.0);

    let track = sequence.track_mut(audio).unwrap();
    track.gain = f32::NAN;
    assert_eq!(track.normalized_gain(), 1.0, "NaN 退回原样");
    track.gain = -1.0;
    assert_eq!(track.normalized_gain(), 1.0, "负数退回原样");
    track.gain = 8.0;
    assert_eq!(
        track.normalized_gain(),
        crate::timeline::MAX_GAIN,
        "超过上限夹到 +6 dB"
    );
    assert_eq!(audio_layers(&sequence, s(0.5))[0].gain, 2.0);
}
