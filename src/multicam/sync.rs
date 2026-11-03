//! 双机位自动对齐：听两路音频，算出右机位要跳过多少开头。
//!
//! 同一个现场的声音在两台机器里只是错开了时间，所以互相关峰出现的位置就是偏移。
//! 用 FFT 算（O(N log N)）：几十秒的采样逐点乘要跑上几分钟，而一次 FFT 只要几十毫秒。
//!
//! 只取两路开头 45 秒：右机位要是提前开机超过这个窗口，共同内容根本不在取样范围里，
//! 那种情况只能手动调。音频统一降到 8kHz 单声道——对齐要的是包络与节奏，不是音质。

use anyhow::{Context as _, Result, anyhow};
use rustfft::{FftPlanner, num_complex::Complex};
use std::path::Path;
use std::process::{Command, Stdio};

/// 取样率。8kHz 足够分辨语音与音乐的包络，样本量也小。
const SAMPLE_RATE: u32 = 8000;
/// 每路取开头的秒数。
const WINDOW_SECONDS: usize = 45;
/// 相关峰至少要比噪声底高出这么多倍标准差，才认为两路是同一场。
const MIN_CONFIDENCE: f32 = 6.0;

/// 对齐结果。
#[derive(Clone, Copy, Debug)]
pub struct Alignment {
    /// 右机位需要跳过的秒数。
    pub skip: f64,
    /// 相关峰的 z 分数。越大说明匹配越可信。
    pub confidence: f32,
}

/// 算出右机位的跳过秒数。失败时给出可执行的原因（没有音轨、太短、对不上）。
pub fn align(left: &Path, right: &Path) -> Result<Alignment> {
    let executable = crate::platform::ffmpeg::executable()?;
    let left_samples = extract(&executable, left)?;
    let right_samples = extract(&executable, right)?;
    let (lag, confidence) = correlate(&left_samples, &right_samples)
        .ok_or_else(|| anyhow!("两路音频都太短，听不出共同内容"))?;
    if confidence < MIN_CONFIDENCE {
        return Err(anyhow!(
            "两路音频对不上（相关峰只有 {confidence:.1} 倍噪声底），\
             可能不是同一场，请手动调整右机位的跳过秒数"
        ));
    }
    Ok(Alignment {
        skip: lag as f64 / SAMPLE_RATE as f64,
        confidence,
    })
}

/// 把一路音频读成 8kHz 单声道 f32 样本。
fn extract(executable: &Path, input: &Path) -> Result<Vec<f32>> {
    let output = Command::new(executable)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-i"])
        .arg(input)
        .args([
            "-vn",
            "-sn",
            "-dn",
            "-ac",
            "1",
            "-ar",
            "8000",
            "-t",
            &WINDOW_SECONDS.to_string(),
            "-f",
            "f32le",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("读取音频失败：{}", input.display()))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "读不出 {} 的音频：{}",
            input.display(),
            detail.trim()
        ));
    }
    let samples: Vec<f32> = output
        .stdout
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    // 不到一秒的样本算不出可靠的峰，宁可报错也不要编一个偏移。
    if samples.len() < SAMPLE_RATE as usize {
        return Err(anyhow!("{} 的音轨太短或没有声音", input.display()));
    }
    Ok(samples)
}

/// 互相关：返回「右机位比左机位多出的开头样本数」与峰的 z 分数。
///
/// `IFFT(A * conj(B))[k] = Σ left[t + k] * right[t]`。右机位先开机时它录到的同一事件
/// 排在更后面（`right[s] = left[s - lag]`），峰落在 `k = -lag` 上，所以最后要取反。
fn correlate(left: &[f32], right: &[f32]) -> Option<(isize, f32)> {
    if left.is_empty() || right.is_empty() {
        return None;
    }
    // 循环卷积的长度要盖住两路之和，否则尾部会绕回来污染真正的峰。
    let size = (left.len() + right.len()).next_power_of_two();
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(size);
    let ifft = planner.plan_fft_inverse(size);

    let mut a = padded(left, size);
    let mut b = padded(right, size);
    fft.process(&mut a);
    fft.process(&mut b);
    let mut product: Vec<Complex<f32>> =
        a.iter().zip(b.iter()).map(|(x, y)| *x * y.conj()).collect();
    ifft.process(&mut product);

    let mut peak = f32::NEG_INFINITY;
    let mut best_lag = 0isize;
    for (index, value) in product.iter().enumerate() {
        // 后半圈的索引对应负 lag。
        let lag = if index <= size / 2 {
            index as isize
        } else {
            index as isize - size as isize
        };
        if value.re > peak {
            peak = value.re;
            best_lag = lag;
        }
    }
    if !peak.is_finite() {
        return None;
    }

    let mean = product.iter().map(|value| value.re).sum::<f32>() / size as f32;
    let variance = product
        .iter()
        .map(|value| (value.re - mean).powi(2))
        .sum::<f32>()
        / size as f32;
    let deviation = variance.sqrt();
    let confidence = if deviation > f32::EPSILON {
        (peak - mean) / deviation
    } else {
        0.0
    };
    Some((-best_lag, confidence))
}

/// 去均值后补零到 `size`。去均值是为了不让直流分量在 0 附近堆一个假峰。
fn padded(samples: &[f32], size: usize) -> Vec<Complex<f32>> {
    let mean = samples.iter().sum::<f32>() / samples.len().max(1) as f32;
    let mut buffer = vec![Complex::new(0.0, 0.0); size];
    for (index, sample) in samples.iter().enumerate() {
        buffer[index] = Complex::new(*sample - mean, 0.0);
    }
    buffer
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 确定性伪噪声：白噪声的互相关峰最尖锐，最适合验证对齐算法。
    fn noise(len: usize, seed: u32) -> Vec<f32> {
        let mut state = seed;
        (0..len)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((state >> 16) as i16) as f32 / 32_768.0
            })
            .collect()
    }

    #[test]
    fn 右机位早开两秒就要跳过两秒() {
        let left = noise(SAMPLE_RATE as usize * 6, 7);
        let mut right = vec![0.0; SAMPLE_RATE as usize * 2];
        right.extend_from_slice(&left);

        let (lag, confidence) = correlate(&left, &right).expect("没找到相关峰");
        assert!(
            (lag as f64 / SAMPLE_RATE as f64 - 2.0).abs() < 0.01,
            "偏移算错：lag = {lag}"
        );
        assert!(confidence > MIN_CONFIDENCE, "置信度过低：{confidence}");
    }

    #[test]
    fn 两路不相干时置信度低() {
        let left = noise(SAMPLE_RATE as usize * 4, 11);
        let right = noise(SAMPLE_RATE as usize * 4, 29);
        let (_, confidence) = correlate(&left, &right).expect("没找到相关峰");
        assert!(confidence < MIN_CONFIDENCE, "噪声也能对上：{confidence}");
    }

    #[test]
    fn 空输入不产生偏移() {
        assert!(correlate(&[], &[]).is_none());
    }
}
