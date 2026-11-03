//! 裁切用的编码器选择：把「编码器模式 + 画质档」翻译成 ffmpeg 参数。
//!
//! 探测与缓存都在 [`crate::encoder`]，与格式转换共用；这里只保留裁切自己的语义：
//! 三档画质换算成码率、强制硬件却探测不到时的报错。

use super::export::{EncoderMode, ExportOptions, Quality, RateControl, VideoCodec};
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
///
/// 三种情况：固定码率（不管软件还是硬件）、硬件编码按档位给目标码率、
/// 软件编码走 CRF（用户自定义的 CRF 优先于画质档）。
pub(super) fn video_args(encoder: &str, options: &ExportOptions) -> Vec<String> {
    if options.rate_control == RateControl::Bitrate {
        let kbps = options.bitrate_kbps.clamp(200, 100_000);
        return encoder::cbr_args(
            encoder,
            &format!("{kbps}k"),
            // 缓冲给两秒：太小会让编码器被迫在每帧之间大幅调整量化，画质发虚。
            &format!("{}k", kbps * 2),
        );
    }
    if encoder::is_hardware(encoder) {
        return encoder::bitrate_args(encoder, bitrate(options.quality));
    }
    let crf = options.crf.unwrap_or_else(|| options.quality.crf());
    vec![
        "-preset".to_string(),
        "medium".to_string(),
        "-crf".to_string(),
        crf.to_string(),
    ]
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

    fn options() -> ExportOptions {
        ExportOptions::default()
    }

    #[test]
    fn 软件编码沿用_crf_硬件编码用目标码率() {
        let high = ExportOptions {
            quality: Quality::High,
            ..options()
        };
        let software = video_args("libx264", &high);
        assert!(software.windows(2).any(|pair| pair == ["-crf", "18"]));

        let nvenc = video_args("hevc_nvenc", &options());
        assert!(nvenc.windows(2).any(|pair| pair == ["-b:v", "5M"]));
        assert!(!nvenc.iter().any(|arg| arg == "-crf"));

        let small = ExportOptions {
            quality: Quality::Small,
            ..options()
        };
        let amf = video_args("h264_amf", &small);
        assert!(amf.windows(2).any(|pair| pair == ["-b:v", "2M"]));
    }

    #[test]
    fn 自定义_crf_优先于画质档() {
        let custom = ExportOptions {
            quality: Quality::High,
            crf: Some(30),
            ..options()
        };
        let args = video_args("libx264", &custom);
        assert!(args.windows(2).any(|pair| pair == ["-crf", "30"]));
    }

    #[test]
    fn 固定码率把上下限都钉住() {
        let cbr = ExportOptions {
            rate_control: RateControl::Bitrate,
            bitrate_kbps: 8_000,
            ..options()
        };
        let nvenc = video_args("h264_nvenc", &cbr);
        assert!(nvenc.windows(2).any(|pair| pair == ["-rc", "cbr"]));
        assert!(nvenc.windows(2).any(|pair| pair == ["-b:v", "8000k"]));
        assert!(nvenc.windows(2).any(|pair| pair == ["-minrate", "8000k"]));
        assert!(nvenc.windows(2).any(|pair| pair == ["-maxrate", "8000k"]));
        assert!(nvenc.windows(2).any(|pair| pair == ["-bufsize", "16000k"]));
        assert!(
            !nvenc.iter().any(|arg| arg == "-crf"),
            "码率模式下不再给 CRF"
        );

        // 软件编码也支持固定码率：libx264 认 VBV 那套参数。
        let software = video_args("libx264", &cbr);
        assert!(
            software
                .windows(2)
                .any(|pair| pair == ["-maxrate", "8000k"])
        );
        assert!(!software.iter().any(|arg| arg == "-rc"));
    }
}
