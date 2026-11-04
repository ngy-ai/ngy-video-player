//! 素材元信息与「文件是否被换过」的判断。
//!
//! 这里只有数据结构与文件指纹，不含 ffmpeg 调用：时间线模块不许依赖 `platform`
//! （见 `TODO.md` §0.5.1）。真正的探测在 `src/media.rs`，读出来再塞进 [`MediaInfo`]。
//!
//! 探测结果随工程存盘，打开工程不必重探；但文件可能被用户换掉，所以同时记下
//! 大小与修改时间。两者任一变化就把缓存标成过期，界面提示重链，而不是拿着旧的
//! 时长继续剪——那会让所有切点整体错位。

use crate::timeline::{Rational, Tick};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

/// 一次探测得到的素材信息。音频文件没有画面，视频相关字段留空。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaInfo {
    /// 时长。容器报 `N/A` 时是 0，调用方据此显示「未知」。
    pub duration: Tick,
    /// 平均帧率。可变帧率素材用它近似；音频文件是 `None`。
    pub fps: Option<Rational>,
    pub width: u32,
    pub height: u32,
    /// 旋转元数据（手机竖拍常见 90 / 270），显示时要按它转。
    pub rotation: u16,
    pub pixel_format: String,
    pub video_codec: String,
    pub color: ColorTags,
    pub audio_codec: String,
    pub channels: u32,
    pub sample_rate: u32,
}

impl MediaInfo {
    pub fn has_video(&self) -> bool {
        self.width > 0 && self.height > 0
    }

    pub fn has_audio(&self) -> bool {
        self.channels > 0
    }

    /// 按旋转元数据换算后的显示尺寸：竖拍素材的 1920×1080 实际是竖着的 1080×1920。
    pub fn display_size(&self) -> (u32, u32) {
        if self.rotation == 90 || self.rotation == 270 {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }
}

/// ffmpeg 打印的色彩标签。
///
/// 不同版本的打印格式不一致（SDR 只给矩阵，HDR 给 `矩阵/原色/传递`），所以原样留下，
/// 不在这里猜顺序。判断 HDR 时看 [`ColorTags::tags`] 里有没有 `smpte2084` / `bt2020`。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorTags {
    /// `tv` / `pc` / `unknown`。
    pub range: String,
    /// `bt709` 或 `bt2020nc/bt2020/smpte2084` 这样的合并写法。
    pub tags: String,
}

/// 文件指纹：大小 + 修改时间（Unix 秒）。
///
/// 不用哈希：几百 MB 的素材每次打开工程都算一遍哈希，代价远大于收益；大小与
/// 修改时间同时不变、内容却变了的情况，在这个工具的使用场景里可以忽略。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaStamp {
    pub size: u64,
    pub modified: i64,
}

impl MediaStamp {
    /// 读文件的指纹。文件不存在或读不到元数据时返回 `None`。
    pub fn of(path: &Path) -> Option<Self> {
        let meta = fs::metadata(path).ok()?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_secs().min(i64::MAX as u64) as i64)
            .unwrap_or(0);
        Some(Self {
            size: meta.len(),
            modified,
        })
    }
}

/// 素材此刻能不能直接拿来剪。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetState {
    /// 文件在、探测缓存可信。
    Ready,
    /// 文件已经不在原地，需要用户重新链接。
    Missing,
    /// 文件被换过（大小或修改时间变了），缓存的时长/帧率不再可信。
    Changed,
}

impl AssetState {
    /// 界面上的提示语。三种状态都给出下一步该做什么。
    pub fn message(self) -> &'static str {
        match self {
            AssetState::Ready => "可用",
            AssetState::Missing => "文件已不在原位置，请重新链接",
            AssetState::Changed => "文件已改动，需要重新探测",
        }
    }
}

#[cfg(test)]
mod tests;
