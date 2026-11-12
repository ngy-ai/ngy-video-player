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
    /// 来自哪个片段。转场过渡区间要按它找接缝。
    pub clip: Id,
    /// 落在片尾定格里：画面被冻在最后一帧，不要再让 mpv 往下播。
    pub frozen: bool,
}

/// 参与输出的视频轨，按叠放顺序（先上后下）。静音 / 隐藏的、空轨都跳过。
pub fn enabled_video_tracks(sequence: &Sequence) -> impl Iterator<Item = &Track> {
    sequence
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video && !track.muted && !track.clips.is_empty())
}

/// 播放头在 `at` 时该显示哪一帧。`None` 表示这一刻没有任何画面（空档）。
///
/// 素材内时刻走 [`crate::timeline::ClipItem::source_at`]，倍率与片尾定格都在那里算：
/// 2 倍速的片段在 0.5 秒处要取素材的 1.0 秒，定格段则一直取最后一帧。
///
/// 单路预览只看「最上面那条」，不透明度在这里不生效——真要按透明度叠，走 [`layers`]。
pub fn resolve(sequence: &Sequence, at: Tick) -> Option<PreviewFrame> {
    enabled_video_tracks(sequence).find_map(|track| frame_at(track, at))
}

/// 一条轨在 `at` 这一刻要出的画面。
fn frame_at(track: &Track, at: Tick) -> Option<PreviewFrame> {
    track.clips.iter().find_map(|clip| {
        let source = clip.source_at(at)?;
        Some(PreviewFrame {
            asset: clip.asset_id,
            source,
            track: track.id,
            clip: clip.id,
            // 定格段里 `source_at` 一直给最后一帧；播放器要知道「别再往下走了」。
            frozen: at - clip.start >= clip.motion(),
        })
    })
}

/// 合成用的一层画面：要放什么 + 这一层有多不透明。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewLayer {
    pub frame: PreviewFrame,
    /// 0 = 完全透明，1 = 完全不透明。取自片段的不透明度关键帧，没记过就是 1。
    pub opacity: f32,
}

/// 播放头在 `at` 时该叠的整套画面，**自下而上**（列表末尾是上面那层）。
///
/// 选轨规则与 [`resolve`] 同一条（静音 / 空轨跳过），区别是把每一层都取出来而不是
/// 只留最上面那条：单路预览解不了两路，多路合成（`player::compositor`）按这份叠。
/// 不透明度为 0 的层不出现——它在成片里也看不见，留着只是白解一路。
pub fn layers(sequence: &Sequence, at: Tick) -> Vec<PreviewLayer> {
    // 轨道列表靠上的画在上面，所以反着走：先底后顶。
    let mut tracks: Vec<&Track> = enabled_video_tracks(sequence).collect();
    tracks.reverse();

    let mut layers = Vec::new();
    for track in tracks {
        let Some(frame) = frame_at(track, at) else {
            continue;
        };
        let clip = track
            .clips
            .iter()
            .find(|clip| clip.id == frame.clip)
            .expect("画面是从这条轨的片段里取出来的");
        // 关键帧的时间是**片段内**的时刻（与导出侧的 `fade` 同一把尺子）。
        let local = (at - clip.start).max(Tick::ZERO);
        let opacity = clip
            .keys
            .value(crate::timeline::Property::Opacity, local)
            .clamp(0.0, 1.0);
        if opacity <= 0.0 {
            continue;
        }
        layers.push(PreviewLayer { frame, opacity });
    }
    layers
}

/// 序列某一时刻该放的声音。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewAudio {
    /// 该放哪段素材。
    pub asset: AssetId,
    /// 素材内部的时刻。
    pub source: Tick,
    /// 来自哪条轨（调试与界面提示用）。
    pub track: Id,
    /// 来自哪个片段。
    pub clip: Id,
    /// 这条轨的音量（线性增益，1.0 = 原样）。预览乘到 mpv 的音量上，与导出侧
    /// `volume` 滤镜同一个量纲。
    pub gain: f32,
}

/// 参与发声的音频轨，按轨道列表顺序（靠上的在前）。静音 / 被独奏挤掉 / 空轨都跳过。
///
/// 预览与导出共用这一条规则（导出侧 `audio_stream` 也用它），两边的「哪些轨在响、
/// 各多大声」因此不会各写一遍。
pub fn enabled_audio_tracks(sequence: &Sequence) -> impl Iterator<Item = &Track> {
    let solo = sequence.audio_solo();
    sequence.tracks.iter().filter(move |track| {
        track.kind == TrackKind::Audio && track.audible(solo) && !track.clips.is_empty()
    })
}

/// 播放头在 `at` 时每条音频轨上该放什么。
///
/// 与 [`layers`] 对应：多轨同时出声（M10-T05）时每条轨各要一路实例，所以这里把
/// 命中的轨都取出来，而不是只留一条。列表顺序与轨道列表一致。
pub fn audio_layers(sequence: &Sequence, at: Tick) -> Vec<PreviewAudio> {
    enabled_audio_tracks(sequence)
        .filter_map(|track| {
            let clip = track
                .clips
                .iter()
                .find(|clip| clip.source_at(at).is_some())?;
            Some(PreviewAudio {
                asset: clip.asset_id,
                source: clip.source_at(at)?,
                track: track.id,
                clip: clip.id,
                gain: track.normalized_gain(),
            })
        })
        .collect()
}

/// 播放头在 `at` 时音频轨上该放哪个片段的哪一刻。`None` 表示没有音频片段。
///
/// 只要一路声音的地方（试听、单路诊断）用这个；多轨同时出声走 [`audio_layers`]。
/// 多条轨同时有内容时取轨道列表里靠上的那条。
pub fn resolve_audio(sequence: &Sequence, at: Tick) -> Option<PreviewAudio> {
    audio_layers(sequence, at).into_iter().next()
}

/// 这一刻画面上该压多厚的黑（0 = 不用压，1 = 全黑）。
///
/// 用来把两件预览做不到的事近似掉：
///
/// - **空档**：没有任何画面时整块压黑，而不是把上一帧留在屏幕上；
/// - **转场**：预览一次只解一路画面，混不出两幅画面的交叉溶解，所以在接缝两侧各取
///   一半时长，先淡到黑再淡回来（接缝那一刻最暗）。
///
/// 变速 / 定格片段之间的转场在导出里会退化成硬切（见 `export::plan`），这里同样
/// 不压黑，免得预览里看到的过渡成片里没有。
pub fn overlay_dim(sequence: &Sequence, at: Tick) -> f32 {
    let Some(frame) = resolve(sequence, at) else {
        return 1.0;
    };
    transition_dim(sequence, frame.track, frame.clip, at)
}

/// 当前片段两侧的接缝里，这一时刻该压多厚的黑。
///
/// 窗口以接缝为中心、两侧各占一半时长。为什么不整段落在接缝之前：那样淡到最黑之后
/// 紧接着就是下一段的全亮，接缝处从全黑跳到全亮，看起来像是闪了一下；跨过接缝被
/// 两段共用，才是「淡出 + 淡入」。
fn transition_dim(sequence: &Sequence, track: Id, clip: Id, at: Tick) -> f32 {
    let Some(track) = sequence.track(track) else {
        return 0.0;
    };
    let Some(index) = track.clip_index(clip) else {
        return 0.0;
    };
    // 播放头在接缝前命中前一段的「后接缝」，在接缝后命中后一段的「前接缝」，
    // 两条接缝的窗口有一半是重叠的，取浓的那个。
    let before = match index.checked_sub(1) {
        Some(previous) => seam_dim(track, previous, at),
        None => 0.0,
    };
    before.max(seam_dim(track, index, at))
}

/// `clips[index]` 与下一个片段之间的接缝上，`at` 该压多黑。
fn seam_dim(track: &Track, index: usize, at: Tick) -> f32 {
    let (Some(before), Some(after)) = (track.clips.get(index), track.clips.get(index + 1)) else {
        return 0.0;
    };
    if before.end() != after.start {
        return 0.0;
    }
    let plain = |clip: &crate::timeline::ClipItem| {
        (clip.rate() - 1.0).abs() < 1e-6 && clip.hold <= Tick::ZERO
    };
    if !plain(before) || !plain(after) {
        return 0.0;
    }
    let Some(transition) = track
        .transitions
        .iter()
        .find(|transition| transition.before == before.id && transition.after == after.id)
    else {
        return 0.0;
    };
    let half = transition.duration.micros() / 2;
    if half <= 0 {
        return 0.0;
    }
    let offset = at.micros() - after.start.micros();
    if offset.abs() > half {
        return 0.0;
    }
    (1.0 - offset.abs() as f32 / half as f32).clamp(0.0, 1.0)
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

/// 播放头往回走 `elapsed` 真实时间（乘倍速），到 `start` 停住。
///
/// 倒放没有「反向解码」这回事：mpv 只能正向解码，所以时间线这边照常按墙钟往回
/// 走，画面靠每一拍补一次 seek 追上来（与拖播放头同一条路径，也因此倒放没有声音）。
pub fn rewind(playhead: Tick, elapsed: Duration, rate: f64, start: Tick) -> Tick {
    let elapsed = elapsed.min(MAX_STEP).as_secs_f64();
    let rate = if rate.is_finite() && rate > 0.0 {
        rate.clamp(0.1, 4.0)
    } else {
        1.0
    };
    playhead
        .saturating_sub(Tick::from_secs(elapsed * rate))
        .max(start)
}

/// J/K/L 的档位：`1x → 2x → 4x`，再按就停在 4x（专业剪辑器的习惯，不循环）。
///
/// `current` 是当前档位（方向由调用方决定，这里只看绝对值），返回下一档。
pub fn shuttle_step(current: f64) -> f64 {
    let current = if current.is_finite() {
        current.abs().clamp(1.0, 4.0)
    } else {
        1.0
    };
    if current <= 1.0 {
        2.0
    } else {
        // 2x 的下一档就是 4x；已经在 4x 时再按也还是 4x，不循环回 1x。
        4.0
    }
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
    fn 合成层自下而上给出每条轨的画面() {
        let mut sequence = sequence();
        let top = sequence.tracks[0].id;
        put(&mut sequence, top, 1, 0.0, 5.0, 0.0);
        let lower = sequence.add_track(TrackKind::Video);
        put(&mut sequence, lower, 2, 0.0, 5.0, 0.0);

        let layers = layers(&sequence, s(1.0));
        assert_eq!(layers.len(), 2, "两条轨各出一层");
        assert_eq!(layers[0].frame.track, lower, "先给底下的那条");
        assert_eq!(layers[1].frame.track, top, "上面那条在末尾");
        assert_eq!(layers[0].opacity, 1.0, "没记过关键帧就是不透明");
    }

    #[test]
    fn 上层的不透明度来自关键帧() {
        let mut sequence = sequence();
        let top = sequence.tracks[0].id;
        put(&mut sequence, top, 1, 0.0, 5.0, 2.0);
        let lower = sequence.add_track(TrackKind::Video);
        put(&mut sequence, lower, 2, 0.0, 5.0, 0.0);

        // 片段从 2 秒开始，关键帧记的是片段内时刻：第 0 秒全透明、第 2 秒不透明。
        let clip = sequence.track_mut(top).unwrap();
        let keys = &mut clip.clips[0].keys;
        keys.ensure(crate::timeline::Property::Opacity)
            .insert(Tick::ZERO, 0.0);
        keys.ensure(crate::timeline::Property::Opacity)
            .insert(s(2.0), 1.0);

        assert_eq!(layers(&sequence, s(2.0)).len(), 1, "全透明的层不出现");
        assert_eq!(layers(&sequence, s(2.0))[0].frame.asset, AssetId::new(2));
        let with_top = layers(&sequence, s(3.0));
        assert_eq!(with_top.len(), 2);
        let opacity = with_top[1].opacity;
        assert!((opacity - 0.5).abs() < 0.05, "半程半透明：{opacity}");
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

    #[test]
    fn 倍速片段按倍率取素材时刻() {
        let mut sequence = sequence();
        let video = sequence.tracks[0].id;
        put(&mut sequence, video, 1, 0.0, 2.0, 0.0);
        // 2 倍速：时间线上的 0.5 秒对应素材的 1.0 秒，片段只占 1 秒。
        sequence.tracks[0].clips[0].speed = 2.0;
        assert_eq!(resolve(&sequence, s(0.5)).unwrap().source, s(1.0));
        assert_eq!(resolve(&sequence, s(1.0)), None, "1 秒之后已经出片段");

        // 0.5 倍速：时间线上的 1 秒只走到素材的 0.5 秒。
        sequence.tracks[0].clips[0].speed = 0.5;
        assert_eq!(resolve(&sequence, s(1.0)).unwrap().source, s(0.5));
        assert_eq!(resolve(&sequence, s(3.9)).unwrap().source, s(1.95));
    }

    #[test]
    fn 定格段停在最后一帧() {
        let mut sequence = sequence();
        let video = sequence.tracks[0].id;
        put(&mut sequence, video, 1, 0.0, 2.0, 0.0);
        sequence.tracks[0].clips[0].hold = s(2.0);

        assert!(
            !resolve(&sequence, s(0.5)).unwrap().frozen,
            "运动段不是定格"
        );
        let frame = resolve(&sequence, s(3.0)).expect("定格段里也有画面");
        assert_eq!(
            frame.source,
            s(2.0) - Tick::from_micros(1),
            "定格是把最后一帧重复出来"
        );
        assert!(frame.frozen, "定格段要告诉播放器别再往下播");
        assert_eq!(resolve(&sequence, s(4.0)), None, "定格结束后才是空档");
    }

    // -- 音频轨 ------------------------------------------------------------

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

    // -- 黑场与转场 --------------------------------------------------------

    /// 两段紧挨着的片段 + 一条 1 秒的转场，返回后一段的起点。
    fn seam(sequence: &mut Sequence) -> (Id, Id, Tick) {
        let video = sequence.tracks[0].id;
        let first = sequence.alloc_id();
        sequence.track_mut(video).unwrap().clips.push(ClipItem::new(
            first,
            AssetId::new(1),
            s(0.0),
            s(2.0),
            s(0.0),
        ));
        let second = sequence.alloc_id();
        sequence.track_mut(video).unwrap().clips.push(ClipItem::new(
            second,
            AssetId::new(2),
            s(1.0),
            s(3.0),
            s(2.0),
        ));
        let transition = sequence.alloc_id();
        sequence
            .track_mut(video)
            .unwrap()
            .transitions
            .push(crate::timeline::Transition::new(
                transition,
                first,
                second,
                crate::timeline::TransitionKind::CrossDissolve,
                s(1.0),
            ));
        (first, second, s(2.0))
    }

    #[test]
    fn 空档整块压黑() {
        let mut sequence = sequence();
        let video = sequence.tracks[0].id;
        put(&mut sequence, video, 1, 0.0, 1.0, 0.0);
        assert_eq!(overlay_dim(&sequence, s(0.5)), 0.0, "有画面时不许压黑");
        assert_eq!(overlay_dim(&sequence, s(1.5)), 1.0, "空档是黑场");
    }

    #[test]
    fn 转场中点最暗两端透明() {
        let mut sequence = sequence();
        let (_, _, seam) = seam(&mut sequence);

        assert_eq!(overlay_dim(&sequence, s(0.9)), 0.0, "窗口之前正常显示");
        assert_eq!(overlay_dim(&sequence, seam - s(0.5)), 0.0, "刚进窗口全亮");
        let middle = overlay_dim(&sequence, seam);
        assert!((middle - 1.0).abs() < 1e-6, "接缝上全黑：{middle}");
        let out = overlay_dim(&sequence, seam - s(0.25));
        assert!((out - 0.5).abs() < 1e-6, "接缝前一拍的淡出半黑：{out}");
        let back = overlay_dim(&sequence, seam + s(0.25));
        assert!(
            (back - 0.5).abs() < 1e-6,
            "接缝后一拍应该淡回来而不是直接全亮：{back}"
        );
        assert_eq!(overlay_dim(&sequence, seam + s(0.5)), 0.0, "窗口之后全亮");
    }

    #[test]
    fn 变速片段之间的转场不演过渡() {
        // 导出侧对变速片段退化成硬切，预览不能演一个成片里没有的过渡。
        let mut sequence = sequence();
        let (_, second, seam) = seam(&mut sequence);
        let track = &mut sequence.tracks[0];
        let index = track.clip_index(second).unwrap();
        track.clips[index].speed = 2.0;

        assert_eq!(overlay_dim(&sequence, seam), 0.0);
        assert_eq!(overlay_dim(&sequence, seam - s(0.25)), 0.0);
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
    fn 倒放按倍速往回走并停在工作区入点() {
        assert_eq!(
            rewind(s(2.0), Duration::from_millis(100), 1.0, s(0.0)),
            s(1.9)
        );
        assert_eq!(
            rewind(s(2.0), Duration::from_millis(100), 2.0, s(0.0)),
            s(1.8)
        );
        assert_eq!(
            rewind(s(10.2), Duration::from_millis(250), 1.0, s(10.0)),
            s(10.0),
            "退到入点就停住"
        );
        // 卡顿上限与正向一致。
        assert_eq!(rewind(s(5.0), Duration::from_secs(2), 1.0, s(0.0)), s(4.75));
    }

    #[test]
    fn 变速档位是一二三档不循环() {
        assert_eq!(
            shuttle_step(0.0),
            2.0,
            "没在播时按一下先给 1x，由调用方处理"
        );
        assert_eq!(shuttle_step(1.0), 2.0);
        assert_eq!(shuttle_step(2.0), 4.0);
        assert_eq!(shuttle_step(4.0), 4.0, "到 4x 就不再往上");
        assert_eq!(shuttle_step(-2.0), 4.0, "只看绝对值，方向由调用方决定");
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
