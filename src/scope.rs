//! 亮度直方图（示波器里最实用的一种）。
//!
//! 不碰解码器内部：让 ffmpeg 在当前位置抽一帧、缩成 160×90 的灰度图，再从
//! stdout 读回 14KB 原始像素统计。比接 mpv 的 `screenshot-raw` 节点简单得多，
//! 也不会和播放线程抢帧。
//!
//! 用途是看曝光：黑场/白场占比高说明暗部死黑或高光过曝，中间调断档说明对比
//! 拉得太狠。真正的波形图/矢量示波器要做二维统计，这里只做一维直方图。

use anyhow::{Context as _, Result};
use std::path::Path;
use std::process::{Command, Stdio};

/// 直方图的柱数。64 根在面板宽度里还能看清，再多就成一片噪点了。
pub const BINS: usize = 64;
/// 抽样分辨率。够统计，又不会让单帧解码变慢。
const SAMPLE_WIDTH: usize = 160;
const SAMPLE_HEIGHT: usize = 90;
/// 最暗/最亮各 2 根柱子算「死黑 / 过曝」。
const CLIP_BINS: usize = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LumaScope {
    pub bins: [u32; BINS],
    pub total: u32,
}

impl Default for LumaScope {
    fn default() -> Self {
        Self {
            bins: [0; BINS],
            total: 0,
        }
    }
}

impl LumaScope {
    /// 从灰度原始像素统计。空输入返回 `None`，调用方据此报「这一帧没画面」。
    pub fn from_gray(pixels: &[u8]) -> Option<Self> {
        if pixels.is_empty() {
            return None;
        }
        let mut bins = [0u32; BINS];
        for pixel in pixels {
            let index = (*pixel as usize * BINS) / 256;
            bins[index.min(BINS - 1)] += 1;
        }
        Some(Self {
            bins,
            total: pixels.len() as u32,
        })
    }

    /// 死黑像素占比（0–1）。
    pub fn black_ratio(&self) -> f32 {
        self.ratio(self.bins[..CLIP_BINS].iter().sum())
    }

    /// 过曝像素占比（0–1）。
    pub fn white_ratio(&self) -> f32 {
        self.ratio(self.bins[BINS - CLIP_BINS..].iter().sum())
    }

    /// 平均亮度（0–1）。
    pub fn mean(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        let weighted: f64 = self
            .bins
            .iter()
            .enumerate()
            .map(|(index, count)| (index as f64 + 0.5) * *count as f64)
            .sum();
        (weighted / self.total as f64 / BINS as f64) as f32
    }

    /// 最高一根柱子的计数，画图时用来归一化高度。
    pub fn peak(&self) -> u32 {
        self.bins.iter().copied().max().unwrap_or(0)
    }

    fn ratio(&self, count: u32) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            count as f32 / self.total as f32
        }
    }
}

/// 在 `time` 处抽一帧统计。
pub fn sample(input: &Path, time: f64) -> Result<LumaScope> {
    let executable = crate::platform::ffmpeg::executable()?;
    let output = Command::new(executable)
        .args(["-hide_banner", "-nostdin", "-loglevel", "error"])
        .arg("-ss")
        .arg(format!("{:.3}", time.max(0.0)))
        .arg("-i")
        .arg(input)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale=160:90:flags=area,format=gray",
            "-f",
            "rawvideo",
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("抽帧失败：{}", input.display()))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail.trim();
        return Err(anyhow::anyhow!(if detail.is_empty() {
            "ffmpeg 抽帧失败".to_string()
        } else {
            format!("ffmpeg 抽帧失败：{detail}")
        }));
    }
    let expected = SAMPLE_WIDTH * SAMPLE_HEIGHT;
    if output.stdout.len() < expected {
        return Err(anyhow::anyhow!(
            "这一帧没有画面（可能已到片尾），把播放位置往前挪一点再试"
        ));
    }
    LumaScope::from_gray(&output.stdout[..expected])
        .ok_or_else(|| anyhow::anyhow!("取到的画面是空的"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 全黑与全白落在两端() {
        let black = LumaScope::from_gray(&[0; 100]).unwrap();
        assert_eq!(black.bins[0], 100);
        assert_eq!(black.black_ratio(), 1.0);
        assert_eq!(black.white_ratio(), 0.0);

        let white = LumaScope::from_gray(&[255; 100]).unwrap();
        assert_eq!(white.bins[BINS - 1], 100);
        assert_eq!(white.white_ratio(), 1.0);
        assert_eq!(white.black_ratio(), 0.0);
    }

    #[test]
    fn 平均亮度按柱心计算() {
        let scope = LumaScope::from_gray(&[0, 255]).unwrap();
        assert!((scope.mean() - 0.5).abs() < 0.01);
        assert_eq!(scope.total, 2);
        assert_eq!(scope.peak(), 1);
    }

    #[test]
    fn 空画面不当成全黑() {
        assert!(LumaScope::from_gray(&[]).is_none());
    }
}
