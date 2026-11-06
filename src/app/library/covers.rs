//! 封面的生成与素材规格文案。

use super::super::*;
use crate::media_cache::{self, ThumbLevel};
use crate::timeline::MediaInfo;
use anyhow::{Context as _, anyhow};
use image::{Frame, ImageBuffer, Rgba};
use std::path::Path;
use std::sync::Arc;

/// 缩略图要生成多大。面板里显示 72 宽，按两倍给高分屏留余量。
const COVER_WIDTH: u32 = 160;

/// 素材规格：视频给分辨率与帧率，音频给采样率与声道数。
pub(crate) fn spec(media: &MediaInfo) -> String {
    if media.has_video() {
        let (width, height) = media.display_size();
        match media.fps {
            Some(fps) => format!("{width}×{height} · {}fps", crate::timeline::rate_label(fps)),
            None => format!("{width}×{height}"),
        }
    } else if media.has_audio() {
        format!(
            "{:.1}kHz · {} 声道",
            f64::from(media.sample_rate) / 1000.0,
            media.channels
        )
    } else {
        "无可用的音视频轨".to_string()
    }
}

/// 封面在面板里按 `COVER_WIDTH` 宽等比缩小；小图不放大，放大只是变糊。
pub(crate) fn cover_size(width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 || width <= COVER_WIDTH {
        return (width.max(1), height.max(1));
    }
    let scaled = (height as f64 * f64::from(COVER_WIDTH) / f64::from(width)).round() as u32;
    (COVER_WIDTH, scaled.max(1))
}

/// 生成一张素材封面：取缓存里 0 秒那块图块的第一格。
///
/// 跑在后台线程上（会启动 ffmpeg）。缓存命中时只是读一张 jpg，不重新解码素材。
pub(crate) fn render_cover(path: &Path) -> Result<Option<Arc<RenderImage>>> {
    render_cover_in(path, &media_cache::cache_root())
}

/// 同上，但缓存根由调用方给。测试用它把缓存写进临时目录，不污染用户缓存。
pub(crate) fn render_cover_in(path: &Path, root: &Path) -> Result<Option<Arc<RenderImage>>> {
    // Medium 档一格正好 160 像素宽，与面板 72 点宽 × 2 倍屏对得上；
    // Small 档只有 80，高分屏上会糊。
    let Some(strip) = media_cache::ensure_strip(path, ThumbLevel::Medium, root, 0)? else {
        // 素材太短或读不出画面：没有封面不是错误，界面显示占位图标。
        return Ok(None);
    };
    let decoded = image::open(&strip.path)
        .with_context(|| format!("读取缩略图失败：{}", strip.path.display()))?;
    let source = decoded.to_rgba8();
    let Some((x, y, width, height)) = media_cache::cell_rect(
        source.width(),
        source.height(),
        strip.columns,
        strip.rows,
        0,
        0,
    ) else {
        return Ok(None);
    };
    let cell = image::imageops::crop_imm(&source, x, y, width, height).to_image();
    let (width, height) = cover_size(cell.width(), cell.height());
    let resized =
        image::imageops::resize(&cell, width, height, image::imageops::FilterType::Triangle);

    // `RenderImage` 收的是 BGRA，`image` 给的是 RGBA，红蓝得换过来，
    // 否则封面整张泛蓝（与截图预览同一套约定）。
    let mut pixels = resized.into_raw();
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let buffer: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_raw(width, height, pixels)
        .ok_or_else(|| anyhow!("封面像素尺寸不匹配"))?;
    Ok(Some(Arc::new(RenderImage::new([Frame::new(buffer)]))))
}
