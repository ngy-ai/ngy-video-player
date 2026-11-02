//! 目标格式：容器 + 编码组合，以及面板要用的查询接口。
//!
//! 表里收的是 [`crate::formats`] 里能播、并且 ffmpeg **真能写出来**的容器，覆盖
//! 现有的全部视频与音频扩展名。写不出来的那些（`ifo`、`divx`、`evo`、`bik`、
//! `smk`、`flc`、`ape`、`tta`、`dsf`、`mid` 之类没有封装器或编码器，`amv` 则对帧
//! 尺寸与音频帧长有硬约束）刻意不列：列出来只会让用户点一次「开始转换」，然后
//! 拿到一条 ffmpeg 报错。
//!
//! 一律重编码，不提供「流复制换容器」：源文件的编码五花八门（HEVC 10bit、AV1、
//! PGS 字幕），流复制常常能写进新容器却播不出来，用户看到的是「转换成功但画面
//! 坏了」。慢一点，但结果可预期。
//!
//! 同一个扩展名可能对应不同封装器（`mts`/`tp` 都是 MPEG-TS，`alac` 走 MP4），
//! 所以每条都显式写 `-f`，不依赖 ffmpeg 按扩展名猜。
//!
//! 表项本身（编码参数、说明）在 `profiles`；这里只管结构与查询。

mod profiles;
#[cfg(test)]
mod tests;

use crate::encoder::EncoderMode;
use profiles::Profile;

/// 目标属于哪一类：视频容器还是纯音频。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetKind {
    Video,
    Audio,
}

impl TargetKind {
    pub fn label(self) -> &'static str {
        match self {
            TargetKind::Video => "视频",
            TargetKind::Audio => "音频",
        }
    }
}

/// 一个可选的目标格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetFormat {
    /// 扩展名，也是面板上显示的名字。
    extension: &'static str,
    /// ffmpeg 的 `-f`。
    muxer: &'static str,
    profile: Profile,
}

/// 搜索用的归一化：去掉空格、点、连字符再比。
///
/// 用户打的是「h264」，说明里写的是「H.264」；打「mpeg4」也不该被「MPEG-4」
/// 挡在门外。
fn normalize(text: &str) -> String {
    text.chars()
        .filter(|ch| ch.is_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect()
}

impl TargetFormat {
    /// 扩展名，也是面板上显示的名字。
    pub fn extension(self) -> &'static str {
        self.extension
    }

    pub fn kind(self) -> TargetKind {
        if self.profile.keeps_video() {
            TargetKind::Video
        } else {
            TargetKind::Audio
        }
    }

    /// 目标容器里是否还留视频轨。纯音频目标会把画面整个丢掉。
    pub fn keeps_video(self) -> bool {
        self.kind() == TargetKind::Video
    }

    /// 面板上那句说明：选之前就该知道代价。
    pub fn hint(self) -> &'static str {
        self.profile.hint
    }

    /// 搜索词是否命中。扩展名、编码说明、视频/音频分类都算。
    ///
    /// 让「mkv」「h264」「无损」都能搜到人想要的那一条：用户记得的往往不是
    /// 扩展名，而是「能装字幕的那个」或者「那个 AVC 的」。
    pub fn matches(self, query: &str) -> bool {
        let query = normalize(query);
        if query.is_empty() {
            return true;
        }
        normalize(self.extension).contains(&query)
            || normalize(self.hint()).contains(&query)
            || normalize(self.kind().label()).contains(&query)
    }

    /// 按扩展名找回一条。设置里存的是扩展名，读回来要用它还原。
    pub fn by_extension(extension: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|format| format.extension == extension)
    }

    pub(crate) fn muxer(self) -> &'static str {
        self.muxer
    }

    /// 视频流参数（含 `-c:v`）。探测结果缓存在进程内，这里只取一次快照；
    /// 强制硬件却探测不到时返回错误，由调用方在批次开始前报给用户。
    pub(crate) fn video_args(self, mode: EncoderMode) -> Result<Vec<String>, String> {
        self.video_args_with(
            mode,
            crate::encoder::pick(crate::encoder::Family::H264),
            crate::encoder::pick(crate::encoder::Family::Hevc),
        )
    }

    /// 传入探测结果，便于测试固定场景。
    fn video_args_with(
        self,
        mode: EncoderMode,
        h264: Option<&'static str>,
        hevc: Option<&'static str>,
    ) -> Result<Vec<String>, String> {
        self.profile.video_args_with(mode, h264, hevc)
    }

    /// 空切片表示这个容器装不下音频，调用方不该映射音轨。
    pub(crate) fn audio_args(self) -> &'static [&'static str] {
        self.profile.audio
    }

    pub(crate) fn copy_subtitles(self) -> bool {
        self.profile.subtitles
    }
}

impl Default for TargetFormat {
    fn default() -> Self {
        // 表头是 mp4：发给别人也能播，最常用的目标。
        Self::ALL[0]
    }
}
