//! 速度曲线到 ffmpeg 参数的翻译：视频的 `setpts` 表达式、音频的分段 `atempo`。
//!
//! 视频侧用一条表达式就够了：`setpts` 允许任意表达式，把累计时间表折成分段线性的
//! 嵌套 `if` 即可（曲线自己的 [`SpeedCurve::setpts_expression`] 已经算好，单位是秒）。
//!
//! 音频侧不行：`atempo` 只认**恒定**倍率。沿时间变化的速率只能切段——每一段用该段
//! 的平均速率放一遍，再 `concat` 接起来。段怎么切见 [`audio_chunks`] 的注释：切段
//! 位置取曲线折点，每段的恒定倍率取「调和中项」，这样每段时长与曲线积分严格相等，
//! 段与段之间不会累积漂移，只有段内部的时刻有偏差。

use super::{SpeedCurve, clamp_rate};

/// 视频侧：`setpts` 的整套滤镜（含滤镜名，可直接串进链里）。
///
/// `setpts` 里的 `PTS` 是**时基单位**（通常是 1/帧率），不是秒。曲线给的表达式以秒
/// 为自变量，所以把占位符 `S` 换成 `(PTS-STARTPTS)*TB`，算完再 `/TB` 换回时基。
/// 表达式里有逗号（`if(lt(...),...)`），必须用单引号包起来，否则会被当成滤镜分隔符。
pub fn setpts_filter(curve: &SpeedCurve, span: f64, fallback: f32) -> Option<String> {
    let expr = curve.setpts_expression(span, fallback)?;
    // 只替换占位符 `S`（表达式里没有别的 S），换进去的那段文本不会被二次替换。
    let expr = expr.replace('S', "((PTS-STARTPTS)*TB)");
    Some(format!("setpts='({expr})/TB'"))
}

/// 音频侧的一块：素材内区间 `[start, end)`（相对源入点，秒）配一个恒定倍率。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioChunk {
    pub start: f64,
    pub end: f64,
    pub rate: f64,
}

/// 把曲线拆成若干「恒定倍率」的音频块。
///
/// 切段位置就是曲线的折点：折点之间速率线性变化，段内偏差最小；段数与用户点的点数
/// 同一量级（通常两三个），滤镜链不会长到看不懂。曲线为空时退化成单块。
pub fn audio_chunks(curve: &SpeedCurve, span: f64, fallback: f32) -> Vec<AudioChunk> {
    if span <= 0.0 {
        return Vec::new();
    }
    if curve.points.is_empty() {
        return vec![AudioChunk {
            start: 0.0,
            end: span,
            rate: f64::from(clamp_rate(fallback)),
        }];
    }
    // 只有落在 (0, span) 里的折点才有资格切段：片段外面的点不参与。
    let mut bounds: Vec<f64> = vec![0.0];
    for point in &curve.points {
        if point.at > 1e-4 && point.at < span {
            bounds.push(point.at);
        }
    }
    bounds.push(span);
    bounds.dedup();

    let mut chunks = Vec::with_capacity(bounds.len() - 1);
    for pair in bounds.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        if end - start <= 1e-6 {
            continue;
        }
        let rate = harmonic_rate(curve.rate_at(start, fallback), curve.rate_at(end, fallback));
        chunks.push(AudioChunk { start, end, rate });
    }
    chunks
}

/// 速率从 `a` 线性变到 `b` 的这一段，用哪个恒定倍率放才让时长等于 `∫ds/rate`。
///
/// 令 `rate(u) = a + (b-a)u`，则 `∫₀¹ du/rate(u) = ln(b/a)/(b-a)`，所以要
/// `1/rate_h = ln(b/a)/(b-a)`，也就是 `rate_h = (b-a)/ln(b/a)`——调和平均的推广。
/// 两端相等（含坏值被夹成同一个数）时直接取该值。
fn harmonic_rate(a: f32, b: f32) -> f64 {
    let (a, b) = (f64::from(clamp_rate(a)), f64::from(clamp_rate(b)));
    if (b - a).abs() < 1e-6 {
        return a;
    }
    (b - a) / (b / a).ln()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve(points: &[(f64, f32)]) -> SpeedCurve {
        let mut curve = SpeedCurve::default();
        for (at, rate) in points {
            curve.insert(*at, *rate);
        }
        curve
    }

    #[test]
    fn 没有曲线时不给视频表达式() {
        assert!(setpts_filter(&SpeedCurve::default(), 4.0, 1.0).is_none());
        // 跨度为零也不给：没有素材长度就没有时间戳可算。
        assert!(setpts_filter(&curve(&[(0.0, 2.0)]), 0.0, 1.0).is_none());
    }

    #[test]
    fn 视频表达式换算成时基单位并加引号() {
        let filter = setpts_filter(&curve(&[(0.0, 1.0), (4.0, 2.0)]), 4.0, 1.0).expect("有表达式");
        assert!(filter.starts_with("setpts='("), "{filter}");
        assert!(filter.ends_with(")/TB'"), "{filter}");
        // 占位符 `S` 一个都不剩，换成了 ffmpeg 的变量。
        assert!(!filter.contains("(S,"), "{filter}");
        assert!(filter.contains("((PTS-STARTPTS)*TB)"), "{filter}");
    }

    #[test]
    fn 音频块按折点切分且覆盖全跨度() {
        let chunks = audio_chunks(&curve(&[(0.0, 2.0), (3.0, 0.5), (6.0, 1.0)]), 8.0, 1.0);
        assert_eq!(chunks.len(), 3, "{chunks:?}");
        assert_eq!(chunks[0].start, 0.0);
        assert_eq!(chunks[chunks.len() - 1].end, 8.0);
        for pair in chunks.windows(2) {
            assert_eq!(pair[0].end, pair[1].start, "块与块首尾相接");
        }
        assert!((chunks[0].rate - harmonic_rate(2.0, 0.5)).abs() < 1e-9);
    }

    #[test]
    fn 曲线在两端之外的点不切段() {
        // 两个点都在负时间 / 超出跨度：只剩一整块。
        let chunks = audio_chunks(&curve(&[(-2.0, 2.0), (12.0, 0.5)]), 4.0, 1.0);
        assert_eq!(chunks.len(), 1, "{chunks:?}");
        assert_eq!(chunks[0].start, 0.0);
        assert_eq!(chunks[0].end, 4.0);
    }

    #[test]
    fn 单点曲线是恒定倍率的一块() {
        let chunks = audio_chunks(&curve(&[(0.0, 0.5)]), 4.0, 1.0);
        assert_eq!(chunks.len(), 1);
        assert!((chunks[0].rate - 0.5).abs() < 1e-9);
        // 没有曲线时退回片段自己的倍率。
        let plain = audio_chunks(&SpeedCurve::default(), 4.0, 2.0);
        assert_eq!(plain.len(), 1);
        assert!((plain[0].rate - 2.0).abs() < 1e-9);
    }

    #[test]
    fn 调和倍率让每块时长等于积分() {
        // 2x → 0.5x 的一块：直观看是「平均 1.25x」，实际要比它慢一点才是真积分。
        let mut curve = SpeedCurve::default();
        curve.insert(0.0, 2.0);
        curve.insert(4.0, 0.5);
        let chunk = audio_chunks(&curve, 4.0, 1.0)[0];
        let by_chunk = 4.0 / chunk.rate;
        // 闭式解：∫₀⁴ ds/(2-0.375s) = ln(4)/0.375。
        let closed = 4.0f64.ln() / 0.375;
        assert!(
            (by_chunk - closed).abs() < 1e-9,
            "块时长 {by_chunk} 与闭式解 {closed} 不等"
        );
        // 与 `timeline_duration` 的中点法（64 段）差在 1 毫秒量级——远小于一帧。
        let by_table = curve.timeline_duration(4.0, 1.0).secs();
        assert!(
            (by_chunk - by_table).abs() < 1e-3,
            "块时长 {by_chunk} 与 64 段积分 {by_table} 差太多"
        );
        // 用算术平均会偏短，这条钉住「别改成算术平均」。
        let arithmetic = 4.0 / ((2.0 + 0.5) / 2.0);
        assert!(
            (by_chunk - arithmetic).abs() > 0.05,
            "{by_chunk} vs {arithmetic}"
        );
    }

    #[test]
    fn 坏倍率按原速处理() {
        let mut curve = SpeedCurve::default();
        curve.insert(0.0, f32::NAN);
        curve.insert(4.0, 0.0);
        let chunks = audio_chunks(&curve, 4.0, 1.0);
        assert!((chunks[0].rate - 1.0).abs() < 1e-9, "{:?}", chunks[0]);
    }
}
