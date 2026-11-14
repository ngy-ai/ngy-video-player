//! 混音台每轨的「有效增益指示」。
//!
//! **不是实时采样电平表**：多路预览由每路 mpv 直接把 PCM 交给系统混音器，Rust 侧拿不到
//! 音频采样（FFI 只有标量属性、没有 `--ao=pcm` / astats 回传）。这里显示的是
//! `preview::audio_layers` 已经算好的三层连乘增益（片段 × 自动化 × 推子），随播放头、
//! 自动化与推子实时变化；静音 / 空档 / 被独奏挤掉的轨不在 `audio_layers` 里，显示为 0。
//!
//! 它回答的是「这一刻这条轨被放大/衰减到多少、有没有越过 0 dB」，不是「素材实际峰值」。
//! 真峰值表需要把预览音频链路改成能回读采样，属于另一次架构改动。

use super::*;
use crate::timeline::preview::audio_layers;
use crate::timeline::{Id, Sequence, Tick};

/// 播放头在 `at` 时这条轨的有效增益；不在发声列表里就是 0。
pub(super) fn track_gain(sequence: &Sequence, track: Id, at: Tick) -> f32 {
    audio_layers(sequence, at)
        .into_iter()
        .find(|layer| layer.track == track)
        .map(|layer| layer.gain)
        .unwrap_or(0.0)
}

/// 增益 → 电平条比例（0..1）。与推子同一套 dB 刻度，所以 0 dB 落在同一个高度。
pub(super) fn meter_level(gain: f32) -> f32 {
    if !gain.is_finite() || gain <= 0.0 {
        return 0.0;
    }
    ratio_from_gain(gain)
}

/// 线性增益超过 1.0（0 dB）算过载：条变红，提醒可能有削波。
pub(super) fn overloaded(gain: f32) -> bool {
    gain.is_finite() && gain > 1.0 + 1e-4
}
