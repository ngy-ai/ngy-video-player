//! 裁切用的编码器选择：把「编码器模式 + 画质档」翻译成 ffmpeg 参数。
//!
//! 探测与缓存都在 [`crate::encoder`]，与格式转换共用；这里只保留裁切自己的语义：
//! 三档画质换算成码率、强制硬件却探测不到时的报错。

use super::export::{EncoderMode, Quality, VideoCodec};
use crate::encoder::{self, Family};

/// 选出这次要用的编码器名。
pub(super) fn pick(codec: VideoCodec, mode: EncoderMode) -> Result<&'static str, String> {
    let available = encoder::available();
    pick_from(codec, mode, &available)
}

fn pick_from(
    codec: VideoCodec,
    mode: EncoderMode,
    available: &[&'static str],
) -> Result<&'static str, String> {
    let hardware = encoder::pick_from(family(codec), available);

    match mode {
        EncoderMode::Software => Ok(codec.software()),
        EncoderMode::Auto => Ok(hardware.unwrap_or_else(|| codec.software())),
        EncoderMode::Hardware => hardware.ok_or_else(|| {
            format!(
                "没有检测到可用的 {} 硬件编码器。\
                 把「编码器」改回「自动」或「软件」即可；\
                 要用显卡编码请确认驱动正常，且 ffmpeg 带 NVENC / QSV / AMF / VideoToolbox。\
                 程序刚启动时探测还要一两秒，稍等再试。",
                codec.label()
            )
        }),
    }
}

fn family(codec: VideoCodec) -> Family {
    match codec {
        VideoCodec::H264 => Family::H264,
        VideoCodec::Hevc => Family::Hevc,
    }
}

/// 编码器对应的输出参数（不含 `-c:v`）。
pub(super) fn video_args(encoder: &str, quality: Quality) -> Vec<String> {
    if !encoder::is_hardware(encoder) {
        return vec![
            "-preset".to_string(),
            "medium".to_string(),
            "-crf".to_string(),
            quality.crf().to_string(),
        ];
    }
    encoder::bitrate_args(encoder, bitrate(quality))
}

fn bitrate(quality: Quality) -> &'static str {
    match quality {
        Quality::High => "10M",
        Quality::Standard => "5M",
        Quality::Small => "2M",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 自动优先硬件_软件强制软件() {
        let available = ["h264_qsv", "h264_nvenc"];
        assert_eq!(
            pick_from(VideoCodec::H264, EncoderMode::Auto, &available).unwrap(),
            "h264_nvenc"
        );
        assert_eq!(
            pick_from(VideoCodec::H264, EncoderMode::Software, &available).unwrap(),
            "libx264"
        );
        // HEVC 只探测到 H.264 的硬件编码器时不能误用。
        assert_eq!(
            pick_from(VideoCodec::Hevc, EncoderMode::Auto, &available).unwrap(),
            "libx265"
        );
    }

    #[test]
    fn 强制硬件但没探测到时报可执行的错误() {
        let error = pick_from(VideoCodec::Hevc, EncoderMode::Hardware, &[]).unwrap_err();
        assert!(error.contains("没有检测到可用的 H.265 硬件编码器"));
        assert!(error.contains("自动"));
        assert!(error.contains("软件"));
    }

    #[test]
    fn 软件编码沿用_crf_硬件编码用目标码率() {
        let software = video_args("libx264", Quality::High);
        assert!(software.windows(2).any(|pair| pair == ["-crf", "18"]));

        let nvenc = video_args("hevc_nvenc", Quality::Standard);
        assert!(nvenc.windows(2).any(|pair| pair == ["-b:v", "5M"]));
        assert!(!nvenc.iter().any(|arg| arg == "-crf"));

        let amf = video_args("h264_amf", Quality::Small);
        assert!(amf.windows(2).any(|pair| pair == ["-b:v", "2M"]));
    }
}
