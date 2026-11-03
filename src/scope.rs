//! 示波器：亮度直方图、波形图、矢量示波器。
//!
//! 不碰解码器内部：让 ffmpeg 在当前位置抽一帧、缩成 160×90 的 RGB 图，再从
//! stdout 读回 43KB 原始像素，在本地算三种统计。比接 mpv 的 `screenshot-raw`
//! 节点简单得多，也不会和播放线程抢帧。
//!
//! 三种视图回答三个不同的问题：
//! * 亮度直方图：曝光有没有被截断（死黑 / 过曝占比）；
//! * 波形图：画面每一列落在什么亮度上（哪一片过曝、肤色是否骑在中灰）；
//! * 矢量示波器：颜色偏到哪个方向（肤色偏红、蓝底过饱和）。
//!
//! 波形与矢量都是二维统计，塞不进 `div` 网格（几千个元素会拖慢渲染），所以直接
//! 画成一张 RGBA 位图，交给 `canvas` 一次性贴上去。

use anyhow::{Context as _, Result};
use std::path::Path;
use std::process::{Command, Stdio};

/// 直方图的柱数。64 根在面板宽度里还能看清，再多就成一片噪点了。
pub const BINS: usize = 64;
/// 抽样分辨率。够统计，又不会让单帧解码变慢。
pub const SAMPLE_WIDTH: usize = 160;
pub const SAMPLE_HEIGHT: usize = 90;
/// 最暗/最亮各 2 根柱子算「死黑 / 过曝」。
const CLIP_BINS: usize = 2;

/// 波形图的纵向分档数：每个亮度档占一行。
const WAVEFORM_ROWS: usize = 64;
/// 矢量示波器在 Cb / Cr 两个方向上的分档数。
const VECTOR_SIZE: usize = 64;
/// 位图放大倍数。统计分辨率只有几十格，直接贴上去每一格只有 1px，看不清。
const BITMAP_SCALE: u32 = 2;

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

/// 一帧画面的三种统计。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeFrame {
    pub luma: LumaScope,
    /// 波形图：`cells[column * WAVEFORM_ROWS + row]`，`row` 0 最暗。
    waveform: Vec<u32>,
    /// 矢量示波器：`cells[cr * VECTOR_SIZE + cb]`。
    vectorscope: Vec<u32>,
}

impl ScopeFrame {
    /// 从紧密排列的 RGB24 像素统计。像素不够一帧时返回 `None`。
    pub fn from_rgb(pixels: &[u8]) -> Option<Self> {
        let expected = SAMPLE_WIDTH * SAMPLE_HEIGHT * 3;
        if pixels.len() < expected {
            return None;
        }
        let mut bins = [0u32; BINS];
        let mut waveform = vec![0u32; SAMPLE_WIDTH * WAVEFORM_ROWS];
        let mut vectorscope = vec![0u32; VECTOR_SIZE * VECTOR_SIZE];
        for (index, chunk) in pixels[..expected].chunks_exact(3).enumerate() {
            let r = chunk[0] as f32;
            let g = chunk[1] as f32;
            let b = chunk[2] as f32;

            // BT.709 的亮度系数。整数近似会把深色压成同一档，这里直接用浮点。
            let luma = (0.2126 * r + 0.7152 * g + 0.0722 * b).clamp(0.0, 255.0);
            bins[(luma as usize * BINS / 256).min(BINS - 1)] += 1;

            let column = index % SAMPLE_WIDTH;
            let row = (luma as usize * WAVEFORM_ROWS / 256).min(WAVEFORM_ROWS - 1);
            waveform[column * WAVEFORM_ROWS + row] += 1;

            // BT.709 的色度平面：0.5 是中灰，偏离多少就是偏色多少。
            let cb = (-0.1146 * r - 0.3854 * g + 0.5 * b) / 255.0 + 0.5;
            let cr = (0.5 * r - 0.4542 * g - 0.0458 * b) / 255.0 + 0.5;
            let cb = (cb.clamp(0.0, 1.0) * (VECTOR_SIZE - 1) as f32).round() as usize;
            let cr = (cr.clamp(0.0, 1.0) * (VECTOR_SIZE - 1) as f32).round() as usize;
            vectorscope[cr * VECTOR_SIZE + cb] += 1;
        }
        Some(Self {
            luma: LumaScope {
                bins,
                total: (SAMPLE_WIDTH * SAMPLE_HEIGHT) as u32,
            },
            waveform,
            vectorscope,
        })
    }

    /// 波形图里最密的一格，画图时用来归一化颜色。
    pub fn waveform_peak(&self) -> u32 {
        self.waveform.iter().copied().max().unwrap_or(0)
    }

    /// 矢量图里最密的一格。
    pub fn vectorscope_peak(&self) -> u32 {
        self.vectorscope.iter().copied().max().unwrap_or(0)
    }

    /// 波形图：横轴对应画面左右，纵轴是亮度，越亮越靠上。
    pub fn waveform_bitmap(&self) -> Bitmap {
        let width = SAMPLE_WIDTH as u32 * BITMAP_SCALE;
        let height = WAVEFORM_ROWS as u32 * BITMAP_SCALE;
        let mut pixels = backdrop(width, height);
        let peak = self.waveform_peak().max(1) as f32;
        for column in 0..SAMPLE_WIDTH {
            for row in 0..WAVEFORM_ROWS {
                let count = self.waveform[column * WAVEFORM_ROWS + row];
                if count == 0 {
                    continue;
                }
                // 开方压过：一个孤立的尖峰不该把其余部分压成看不见。
                let ratio = (count as f32 / peak).sqrt();
                let left = column as u32 * BITMAP_SCALE;
                let top = (WAVEFORM_ROWS - 1 - row) as u32 * BITMAP_SCALE;
                put_block(&mut pixels, width, left, top, wave_color(ratio));
            }
        }
        Bitmap {
            width,
            height,
            pixels,
        }
    }

    /// 矢量示波器：横轴 Cb、纵轴 Cr，中心是无彩色。
    pub fn vectorscope_bitmap(&self) -> Bitmap {
        let width = VECTOR_SIZE as u32 * BITMAP_SCALE;
        let height = width;
        let mut pixels = backdrop(width, height);
        draw_axes(&mut pixels, width, height);
        let peak = self.vectorscope_peak().max(1) as f32;
        for cr in 0..VECTOR_SIZE {
            for cb in 0..VECTOR_SIZE {
                let count = self.vectorscope[cr * VECTOR_SIZE + cb];
                if count == 0 {
                    continue;
                }
                let ratio = (count as f32 / peak).sqrt();
                let left = cb as u32 * BITMAP_SCALE;
                // 习惯上 Cr 向上为正，和屏幕坐标反过来。
                let top = (VECTOR_SIZE - 1 - cr) as u32 * BITMAP_SCALE;
                put_block(&mut pixels, width, left, top, chroma_color(cb, cr, ratio));
            }
        }
        Bitmap {
            width,
            height,
            pixels,
        }
    }
}

/// 一张紧密排列的 RGBA 位图，界面层只需要把它包成 `RenderImage`。
#[derive(Clone, Debug)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// 在 `time` 处抽一帧统计。
pub fn sample(input: &Path, time: f64) -> Result<ScopeFrame> {
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
            "scale=160:90:flags=area,format=rgb24",
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
    let expected = SAMPLE_WIDTH * SAMPLE_HEIGHT * 3;
    if output.stdout.len() < expected {
        return Err(anyhow::anyhow!(
            "这一帧没有画面（可能已到片尾），把播放位置往前挪一点再试"
        ));
    }
    ScopeFrame::from_rgb(&output.stdout[..expected])
        .ok_or_else(|| anyhow::anyhow!("取到的画面是空的"))
}

/// 示波器的底色，和面板里其它卡片一致。
fn backdrop(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    for chunk in pixels.chunks_exact_mut(4) {
        chunk.copy_from_slice(&[0x0f, 0x0f, 0x13, 0xff]);
    }
    pixels
}

/// 填一个 `BITMAP_SCALE` 见方的块。调用方保证不会越界。
fn put_block(pixels: &mut [u8], width: u32, left: u32, top: u32, color: [u8; 4]) {
    for dy in 0..BITMAP_SCALE {
        let row = ((top + dy) * width + left) as usize * 4;
        for dx in 0..BITMAP_SCALE {
            let index = row + dx as usize * 4;
            pixels[index..index + 4].copy_from_slice(&color);
        }
    }
}

/// 矢量图的十字线：无彩色就在交点上，偏色一眼能看出往哪边跑。
fn draw_axes(pixels: &mut [u8], width: u32, height: u32) {
    let color = [0x33, 0x33, 0x3c, 0xff];
    let mid_x = width / 2;
    for y in 0..height {
        let index = (y * width + mid_x) as usize * 4;
        pixels[index..index + 4].copy_from_slice(&color);
    }
    let mid_y = height / 2;
    for x in 0..width {
        let index = (mid_y * width + x) as usize * 4;
        pixels[index..index + 4].copy_from_slice(&color);
    }
}

/// 波形配色：暗青到亮青。单色比彩虹色更容易读懂密度。
fn wave_color(ratio: f32) -> [u8; 4] {
    let k = 0.35 + 0.65 * ratio;
    [(64.0 * k) as u8, (211.0 * k) as u8, (190.0 * k) as u8, 0xff]
}

/// 矢量图的点按它代表的颜色着色：偏红的地方就是红的，比单色图直观。
fn chroma_color(cb: usize, cr: usize, ratio: f32) -> [u8; 4] {
    let cb = cb as f32 / (VECTOR_SIZE - 1) as f32;
    let cr = cr as f32 / (VECTOR_SIZE - 1) as f32;
    // BT.709 逆变换，Y 取中灰：这里只关心色相，亮度由 ratio 决定。
    let r = 0.5 + 1.5748 * (cr - 0.5);
    let g = 0.5 - 0.1873 * (cb - 0.5) - 0.4681 * (cr - 0.5);
    let b = 0.5 + 1.8556 * (cb - 0.5);
    let k = 0.35 + 0.65 * ratio;
    [
        (r.clamp(0.0, 1.0) * 255.0 * k) as u8,
        (g.clamp(0.0, 1.0) * 255.0 * k) as u8,
        (b.clamp(0.0, 1.0) * 255.0 * k) as u8,
        0xff,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 整帧同一个颜色。
    fn solid(rgb: [u8; 3]) -> Vec<u8> {
        [rgb; SAMPLE_WIDTH * SAMPLE_HEIGHT].concat()
    }

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
        assert!(ScopeFrame::from_rgb(&[]).is_none());
    }

    #[test]
    fn 波形图把黑压在最底一行白顶到最上一行() {
        let black = ScopeFrame::from_rgb(&solid([0, 0, 0])).unwrap();
        for column in 0..SAMPLE_WIDTH {
            assert!(
                black.waveform[column * WAVEFORM_ROWS] > 0,
                "第 {column} 列没落在最暗档"
            );
            assert_eq!(
                black.waveform[column * WAVEFORM_ROWS + WAVEFORM_ROWS - 1],
                0
            );
        }

        let white = ScopeFrame::from_rgb(&solid([255, 255, 255])).unwrap();
        for column in 0..SAMPLE_WIDTH {
            assert_eq!(white.waveform[column * WAVEFORM_ROWS], 0);
            assert!(white.waveform[column * WAVEFORM_ROWS + WAVEFORM_ROWS - 1] > 0);
        }
    }

    #[test]
    fn 矢量图把纯红放到右上偏色区() {
        let red = ScopeFrame::from_rgb(&solid([255, 0, 0])).unwrap();
        // 纯红的 Cr 接近 1、Cb 偏小：落在右上那片。
        let middle = VECTOR_SIZE / 2;
        let in_red = (middle..VECTOR_SIZE)
            .flat_map(|cr| (0..middle).map(move |cb| cr * VECTOR_SIZE + cb))
            .any(|index| red.vectorscope[index] > 0);
        assert!(in_red, "纯红没有落在 Cr 高、Cb 低的区域");

        // 无彩色落在中心交点上。
        let gray = ScopeFrame::from_rgb(&solid([128, 128, 128])).unwrap();
        assert!(gray.vectorscope[middle * VECTOR_SIZE + middle] > 0);
    }

    #[test]
    fn 位图尺寸对齐且不透明() {
        let frame = ScopeFrame::from_rgb(&solid([128, 128, 128])).unwrap();
        let waveform = frame.waveform_bitmap();
        assert_eq!(waveform.width, SAMPLE_WIDTH as u32 * BITMAP_SCALE);
        assert_eq!(waveform.height, WAVEFORM_ROWS as u32 * BITMAP_SCALE);
        assert_eq!(
            waveform.pixels.len(),
            (waveform.width * waveform.height * 4) as usize
        );
        assert!(waveform.pixels.chunks_exact(4).all(|p| p[3] == 255));

        let vectorscope = frame.vectorscope_bitmap();
        assert_eq!(vectorscope.width, VECTOR_SIZE as u32 * BITMAP_SCALE);
        assert_eq!(vectorscope.width, vectorscope.height);
        assert!(vectorscope.pixels.chunks_exact(4).all(|p| p[3] == 255));
    }
}
