//! 速度曲线与时间重映射（M10-T11 的模型层）。
//!
//! 现在的变速是一个档位（`ClipItem.speed` 是常量倍率，改档在 `timeline::speed`）。
//! 速度曲线把它换成**随素材位置变化**的倍率：`rate(s)` 表示「素材走到 s 这一秒时，
//! 播放速度是几倍」。
//!
//! 关键的一点是**用素材位置做自变量**，不是时间线位置：
//!
//! ```text
//! dt_时间线 = ds_素材 / rate(s)
//! ```
//!
//! 反过来讲，时间线时长是 `∫ ds / rate(s)`。这样定义的好处是裁剪片段（改
//! `source_in` / `source_out`）不需要重算曲线：曲线挂在素材位置上，剪掉一段只是
//! 积分区间变了。
//!
//! 积分用中点法分 [`STEPS`] 段：速度曲线是分段线性，段数够多时误差在 1 毫秒以内，
//! 而闭式解要处理「零点附近 rate 趋近 0」的分支，得不偿失。
//!
//! 这里给数字（时长、正反解位置）；翻成 ffmpeg 参数的部分在 `remap/ffmpeg.rs`
//! （`setpts` 表达式与音频分段），由导出侧调用。

mod ffmpeg;

pub use ffmpeg::{AudioChunk, audio_chunks, setpts_filter};

use crate::timeline::Tick;
use serde::{Deserialize, Serialize};

/// 倍率量程，与 `ClipItem` 的档位一致。
pub const MIN_RATE: f32 = 0.25;
pub const MAX_RATE: f32 = 4.0;

/// 积分分段数。64 段在 10 分钟的曲线上也只要几十微秒。
pub const STEPS: usize = 64;

/// 曲线上一个点：素材内相对入点的秒数 + 该处的倍率。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpeedPoint {
    /// 相对**片段的源入点**（0 = 入点那一帧），单位秒。
    pub at: f64,
    pub rate: f32,
}

impl SpeedPoint {
    pub fn new(at: f64, rate: f32) -> Self {
        Self {
            at: at.max(0.0),
            rate: clamp_rate(rate),
        }
    }
}

/// 一条速度曲线。空表示「没有曲线，用片段自己的倍率」。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpeedCurve {
    /// 按 `at` 升序，同一位置只留一个。
    pub points: Vec<SpeedPoint>,
}

impl SpeedCurve {
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// 记一个点（同一位置覆盖）。与关键帧的语义一致，界面按「在播放头记一个点」用。
    pub fn insert(&mut self, at: f64, rate: f32) -> bool {
        let point = SpeedPoint::new(at, rate);
        if let Some(existing) = self
            .points
            .iter_mut()
            .find(|it| (it.at - point.at).abs() < 1e-4)
        {
            existing.rate = point.rate;
            return false;
        }
        self.points.push(point);
        self.points.sort_by(|a, b| a.at.total_cmp(&b.at));
        true
    }

    /// 素材偏移 `offset`（秒，相对源入点）处的倍率。两端之外保持端点值。
    pub fn rate_at(&self, offset: f64, fallback: f32) -> f32 {
        if self.points.is_empty() {
            return clamp_rate(fallback);
        }
        let first = self.points[0];
        if offset <= first.at {
            return first.rate;
        }
        let last = self.points[self.points.len() - 1];
        if offset >= last.at {
            return last.rate;
        }
        let index = self
            .points
            .partition_point(|point| point.at <= offset)
            .saturating_sub(1);
        let (from, to) = (self.points[index], self.points[index + 1]);
        let span = to.at - from.at;
        if span <= 0.0 {
            return to.rate;
        }
        let progress = ((offset - from.at) / span) as f32;
        clamp_rate(from.rate + (to.rate - from.rate) * progress)
    }

    /// 素材跨 `span` 秒（从源入点起）在这条曲线上放多久。
    pub fn timeline_duration(&self, span: f64, fallback: f32) -> Tick {
        if span <= 0.0 {
            return Tick::ZERO;
        }
        if self.points.is_empty() {
            return Tick::from_secs(span / f64::from(clamp_rate(fallback)));
        }
        let step = span / STEPS as f64;
        let mut total = 0.0;
        for index in 0..STEPS {
            // 中点法：分段线性的速度取中点最准。
            let middle = (index as f64 + 0.5) * step;
            total += step / f64::from(self.rate_at(middle, fallback));
        }
        Tick::from_secs(total)
    }

    /// 反解：时间线走过 `elapsed` 秒时，素材走到第几秒（相对源入点）。
    ///
    /// 逐段累加而不是二分：分段线性时每一段的积分有闭式解
    /// （`∫ ds/(a+ks) = ln(...)/k`），但 a 可能贴着 0，逐段数值累加更稳。
    pub fn source_at_timeline(&self, elapsed: f64, span: f64, fallback: f32) -> f64 {
        if elapsed <= 0.0 || span <= 0.0 {
            return 0.0;
        }
        if self.points.is_empty() {
            return (elapsed * f64::from(clamp_rate(fallback))).min(span);
        }
        let step = span / STEPS as f64;
        let mut remaining = elapsed;
        let mut consumed = 0.0;
        for index in 0..STEPS {
            let middle = (index as f64 + 0.5) * step;
            let rate = f64::from(self.rate_at(middle, fallback));
            let cost = step / rate;
            if remaining <= cost {
                // 段内还按段速走：误差是段内变化量的一半，视觉上察觉不到。
                return (consumed + remaining * rate).min(span);
            }
            remaining -= cost;
            consumed += step;
        }
        span
    }
    /// 正解：素材走到 `offset` 秒（相对源入点）时，时间线那边走到第几秒。
    ///
    /// 与 [`Self::setpts_expression`] 用同一张 64 段累计表并做段内插值：预览的播放头
    /// 位置与成片的时间戳因此出自同一套数，不会一个按表算、一个按闭式解算。
    pub fn timeline_at_source(&self, offset: f64, span: f64, fallback: f32) -> f64 {
        if span <= 0.0 {
            return 0.0;
        }
        let rate = f64::from(clamp_rate(fallback));
        if self.points.is_empty() {
            return offset / rate;
        }
        let offset = offset.clamp(0.0, span);
        let table = self.cumulative(span, fallback);
        // 表是按素材位置等分的，直接算出落在哪一段，再在段内线性插值。
        let step = span / STEPS as f64;
        let index = ((offset / step).floor() as usize).min(STEPS - 1);
        let (from, to) = (table[index], table[index + 1]);
        let width = to.0 - from.0;
        if width <= 0.0 {
            return to.1;
        }
        from.1 + (offset - from.0) / width * (to.1 - from.1)
    }

    /// 截出素材内 `[from, to]` 这一段，新曲线的自变量从 0 起。
    ///
    /// 切开与修剪要用：两段各自按切点之后的曲线走，时长才和原来那一段对得上。
    /// 折点之间是线性的，所以补两个端点就保形；端点速率按原曲线插值取。
    pub fn slice(&self, from: f64, to: f64) -> Self {
        if self.points.is_empty() || to - from <= 1e-6 {
            return Self::default();
        }
        let from = from.max(0.0);
        if to <= from {
            return Self::default();
        }
        let mut curve = Self::default();
        curve.insert(0.0, self.rate_at(from, 1.0));
        for point in &self.points {
            if point.at > from + 1e-6 && point.at < to - 1e-6 {
                curve.insert(point.at - from, point.rate);
            }
        }
        curve.insert(to - from, self.rate_at(to, 1.0));
        curve
    }

    /// 曲线上的累计时间表：`(素材位置秒, 时间线秒)`，从 0 到 `span`。
    ///
    /// 导出侧要把它折成分段线性的 `setpts` 表达式；预览侧要拿它做「素材位置 → 时间线位置」
    /// 的换算。两边共用这张表，才不会一个按 64 段算、一个按闭式解算。
    pub fn cumulative(&self, span: f64, fallback: f32) -> Vec<(f64, f64)> {
        let mut table = Vec::with_capacity(STEPS + 1);
        if span <= 0.0 {
            table.push((0.0, 0.0));
            return table;
        }
        let step = span / STEPS as f64;
        let mut elapsed = 0.0;
        table.push((0.0, 0.0));
        for index in 0..STEPS {
            let middle = (index as f64 + 0.5) * step;
            elapsed += step / f64::from(self.rate_at(middle, fallback));
            table.push(((index + 1) as f64 * step, elapsed));
        }
        table
    }

    /// 把累计时间表写成 `setpts` 能用的表达式（`S` 是素材位置的占位符）。
    ///
    /// 分段线性 + 嵌套 `if`：段数与 [`STEPS`] 一致，误差就是中点法的误差（< 1 帧）。
    /// 导出侧替换占位符为 `(PTS-STARTPTS)` 即可，这里不写死 ffmpeg 的变量名，
    /// 单测才能脱离滤镜图验证。
    pub fn setpts_expression(&self, span: f64, fallback: f32) -> Option<String> {
        if self.points.is_empty() || span <= 0.0 {
            return None;
        }
        let table = self.cumulative(span, fallback);
        let mut expr = format!("{:.6}", table[table.len() - 1].1);
        for pair in table.windows(2).rev() {
            let (from, to) = (pair[0], pair[1]);
            let width = to.0 - from.0;
            if width <= 0.0 {
                continue;
            }
            let weight = format!("((S-{:.6})/{width:.6})", from.0);
            expr = format!(
                "if(lt(S,{:.6}),{:.6}+({:.6})*{weight},{expr})",
                to.0,
                from.1,
                to.1 - from.1
            );
        }
        Some(expr)
    }
}

/// 倍率夹进量程；坏值退回 1.0（原速），不是 0（会除爆）。
pub fn clamp_rate(rate: f32) -> f32 {
    if rate.is_finite() && rate > 0.0 {
        rate.clamp(MIN_RATE, MAX_RATE)
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests;
