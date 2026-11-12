//! 预览与导出的选声规则：哪些音频轨在响、各取素材的哪一刻、各多大声。
//!
//! 导出的 `audio_stream` 也用这里的 [`enabled_audio_tracks`]：两边的「哪些轨参与输出、
//! 音量多少」因此不会各写一遍、慢慢长歪（与 [`super::enabled_video_tracks`] 同一个思路）。

use super::*;

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
/// 与 [`super::layers`] 对应：多轨同时出声（M10-T05）时每条轨各要一路实例，所以这里把
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

#[cfg(test)]
mod tests;
