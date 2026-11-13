//! 混音：轨道推子、声像与音量自动化（M10-T14 的模型层）。
//!
//! 三层音量**叠加顺序固定**，从内到外：
//!
//! 1. 片段自己的音量（`ClipItem.gain`，非线性素材的补偿也走它）；
//! 2. 轨道音量自动化（`Track.automation`，随时间走的那条线）；
//! 3. 轨道推子（`Track.gain`，混音台上那个滑块）。
//!
//! 三条都是**线性振幅倍数**，所以合成就是连乘；换成 dB 再相加只在展示时做——
//! 用 dB 做中间量会让「静音」变成 `-inf`，一路传下去到处要写特判。
//!
//! 声像用等功率（constant power）法则：中间时左右各乘 `1/√2`（约 -3 dB），
//! 推到一边时另一边为 0。等功率比「线性衰减」更接近听感，与一般 DAW 的默认一致。

use crate::timeline::{Tick, Track};
use serde::{Deserialize, Serialize};

/// 增益上限（+6 dB）。与 `model::track::MAX_GAIN` 同一个含义：轨道推子、片段音量与
/// 自动化点都受它约束，混音台上没人把一条轨推到 +6 dB 以上。
pub const MAX_GAIN: f32 = 2.0;

/// 线性增益的默认值（原样）。`f32` 的 `Default` 是 0.0 = 全静音，不能直接拿来兜底。
pub fn unity_gain() -> f32 {
    1.0
}

/// 声像范围：-1 全左、0 居中、1 全右。
pub const PAN_LIMIT: f32 = 1.0;

/// 一个自动化点：轨道时间上的线性增益。
///
/// 只做线性（没有贝塞尔）：音量线在混音台上几乎总是分段直线，
/// 加曲线只会让「这一秒到底是几 dB」变得说不清。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GainPoint {
    pub at: Tick,
    /// 线性振幅倍数，0 = 静音。
    pub gain: f32,
}

impl GainPoint {
    pub fn new(at: Tick, gain: f32) -> Self {
        Self {
            at,
            gain: gain.clamp(0.0, crate::timeline::MAX_GAIN),
        }
    }
}

/// 轨道的声像值先夹进合法范围（工程是明文 JSON，手改什么都可能）。
pub fn normalized_pan(pan: f32) -> f32 {
    if pan.is_finite() {
        pan.clamp(-PAN_LIMIT, PAN_LIMIT)
    } else {
        0.0
    }
}

/// 声像 → 左右两路增益（等功率）。
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let pan = normalized_pan(pan);
    // 把 -1–1 映射到 0–π/2，再取余弦 / 正弦：两边平方和恒为 1。
    let angle = (pan + 1.0) * 0.25 * std::f32::consts::PI;
    (angle.cos(), angle.sin())
}

/// 自动化在 `at` 处的增益；没有自动化点时返回 `None`（表示「用推子原样」）。
///
/// 点外不沿用端点值：自动化是混音台上的「这条线」，线没画到的地方就该回到推子，
/// 否则最后一段会一直保持最后一个点，用户会以为推子坏了。
pub fn automation_at(points: &[GainPoint], at: Tick) -> Option<f32> {
    if points.is_empty() {
        return None;
    }
    let mut sorted: Vec<GainPoint> = points.to_vec();
    sorted.sort_by_key(|point| point.at);
    let first = sorted[0];
    if at <= first.at {
        return Some(first.gain);
    }
    let last = sorted[sorted.len() - 1];
    if at >= last.at {
        return Some(last.gain);
    }
    let index = sorted
        .partition_point(|point| point.at <= at)
        .saturating_sub(1);
    let (from, to) = (sorted[index], sorted[index + 1]);
    let span = (to.at - from.at).micros();
    if span <= 0 {
        return Some(to.gain);
    }
    let progress = (at - from.at).micros() as f32 / span as f32;
    Some(from.gain + (to.gain - from.gain) * progress)
}

/// 片段的音量夹进合法范围（坏值按 1.0）。
pub fn clip_gain(gain: f32) -> f32 {
    if gain.is_finite() && gain >= 0.0 {
        gain.min(MAX_GAIN)
    } else {
        1.0
    }
}

/// 这一刻这条轨的线性增益：片段 × 自动化 × 推子。
///
/// `clip_gain` 传片段的音量，`at` 是**轨道时间**（不是片段内时间）。
pub fn linear_gain(track: &Track, clip: f32, at: Tick) -> f32 {
    clip_gain(clip) * automation_at(&track.automation, at).unwrap_or(1.0) * track.normalized_gain()
}

/// 线性增益 → dB 文案。0 与负值写「静音」。
pub fn db_label(gain: f32) -> String {
    if !gain.is_finite() || gain <= 0.0 {
        return "静音".to_string();
    }
    format!("{:+.1} dB", 20.0 * gain.log10())
}

/// 把自动化写成 ffmpeg `volume` 能用的表达式（`eval=frame`）。
///
/// `offset` 是这段素材在轨道上的起点：滤镜链里每段都从 PTS 0 重新开始，
/// 而自动化用的是轨道时间，所以要加回这个偏移。没有自动化时返回 `None`。
pub fn gain_expression(track: &Track, offset: Tick) -> Option<String> {
    let mut points = track.automation.clone();
    if points.is_empty() {
        return None;
    }
    points.sort_by_key(|point| point.at);
    if points.len() == 1 {
        return Some(format!("{:.6}", points[0].gain));
    }
    let at = |tick: Tick| format!("{:.6}", tick.secs() - offset.secs());
    let last = points[points.len() - 1];
    let mut expr = format!("{:.6}", last.gain);
    for pair in points.windows(2).rev() {
        let (from, to) = (pair[0], pair[1]);
        let span = (to.at - from.at).secs();
        if span <= 0.0 {
            continue;
        }
        let weight = format!("((t-{})/{span:.6})", at(from.at));
        expr = format!(
            "if(lt(t,{:.6}),{:.6}+({:.6})*{weight},{expr})",
            at(to.at),
            from.gain,
            to.gain - from.gain
        );
    }
    Some(expr)
}

#[cfg(test)]
mod tests;
