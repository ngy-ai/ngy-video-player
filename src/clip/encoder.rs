//! 视频编码器选择：优先硬件，找不到就回退软件。
//!
//! 探测只跑一次 `ffmpeg -encoders`（进程内缓存）。这份 ffmpeg 是随程序分发的，
//! 有没有 NVENC/QSV/AMF 在编译期不知道，只能运行时问它。
//!
//! 硬件编码器一律用码率控制：同一套 CRF 参数在 NVENC / QSV / AMF / VideoToolbox
//! 上含义不同，有的甚至不认 `-crf`，直接报错。三档画质换算成固定码率更稳，
//! 用户也能预期文件大小。

use super::export::{EncoderMode, Quality, VideoCodec};
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

/// 认识的硬件编码器，顺序就是优先级。
///
/// 同一档里 NVIDIA 放最前：它跨三个平台都有，驱动成熟；QSV 次之；AMF 只在
/// Windows；VideoToolbox 只在 macOS。真正可用的由探测结果决定。
const KNOWN: &[&str] = &[
    "h264_nvenc",
    "hevc_nvenc",
    "h264_qsv",
    "hevc_qsv",
    "h264_amf",
    "hevc_amf",
    "h264_videotoolbox",
    "hevc_videotoolbox",
];

/// 启动后台探测。放在应用启动时调用：探测要起好几个 ffmpeg 进程，
/// 不能在用户点「开始裁切」的那一刻卡住界面。
pub(crate) fn warm_up() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        std::thread::spawn(|| {
            let names = probe_all();
            *AVAILABLE.lock().unwrap() = Some(names);
        });
    });
}

/// 选出这次要用的编码器名。
pub(super) fn pick(codec: VideoCodec, mode: EncoderMode) -> Result<&'static str, String> {
    let available = AVAILABLE.lock().unwrap().clone().unwrap_or_default();
    pick_from(codec, mode, &available)
}

fn pick_from(
    codec: VideoCodec,
    mode: EncoderMode,
    available: &[&'static str],
) -> Result<&'static str, String> {
    let hardware = candidates(codec)
        .iter()
        .copied()
        .find(|name| available.contains(name));

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

fn candidates(codec: VideoCodec) -> &'static [&'static str] {
    match codec {
        VideoCodec::H264 => &["h264_nvenc", "h264_qsv", "h264_amf", "h264_videotoolbox"],
        VideoCodec::Hevc => &["hevc_nvenc", "hevc_qsv", "hevc_amf", "hevc_videotoolbox"],
    }
}

/// 探测结果：启动后由 [`warm_up`] 填好；`None` 表示还在探测。
static AVAILABLE: Mutex<Option<Vec<&'static str>>> = Mutex::new(None);

fn probe_all() -> Vec<&'static str> {
    let Ok(executable) = crate::platform::ffmpeg::executable() else {
        return Vec::new();
    };
    let Ok(output) = Command::new(&executable)
        .args(["-hide_banner", "-encoders"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    parse(&String::from_utf8_lossy(&output.stdout))
        .into_iter()
        .filter(|name| test_encode(&executable, name))
        .collect()
}

/// 编译进去 ≠ 这台机器能用：没有 NVIDIA 显卡时 ffmpeg 仍会列出 `h264_nvenc`，
/// 真正初始化编码器时才失败。用 1 帧 64×64 黑画面试一次，几百毫秒内出结果。
fn test_encode(executable: &Path, name: &str) -> bool {
    Command::new(executable)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=64x64:r=1",
            "-frames:v",
            "1",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            name,
            "-f",
            "null",
            "-",
        ])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// 从 `-encoders` 的表格里挑出认识的编码器。
fn parse(text: &str) -> Vec<&'static str> {
    KNOWN
        .iter()
        .copied()
        .filter(|name| {
            text.lines()
                .any(|line| line.split_whitespace().any(|token| token == *name))
        })
        .collect()
}

/// 编码器对应的输出参数（不含 `-c:v`）。
pub(super) fn video_args(encoder: &str, quality: Quality) -> Vec<String> {
    if !is_hardware(encoder) {
        return vec![
            "-preset".to_string(),
            "medium".to_string(),
            "-crf".to_string(),
            quality.crf().to_string(),
        ];
    }

    let bitrate = match quality {
        Quality::High => "10M",
        Quality::Standard => "5M",
        Quality::Small => "2M",
    };
    let args: &[&str] = if encoder.ends_with("_nvenc") {
        &["-rc", "vbr", "-b:v", bitrate, "-preset", "p5"]
    } else if encoder.ends_with("_qsv") {
        &["-preset", "medium", "-b:v", bitrate]
    } else if encoder.ends_with("_amf") {
        &["-rc", "vbr_peak", "-b:v", bitrate]
    } else {
        // VideoToolbox 与其它未列出的硬件编码器：只给目标码率。
        &["-b:v", bitrate]
    };
    args.iter().map(|arg| arg.to_string()).collect()
}

pub(super) fn is_hardware(encoder: &str) -> bool {
    KNOWN.contains(&encoder)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "\
Encoders:
 V..... = Video
 ------
 V....D libx264              libx264 H.264 / AVC / MPEG-4 AVC (codec h264)
 V....D h264_nvenc           NVIDIA NVENC H.264 encoder (codec h264)
 V....D hevc_nvenc           NVIDIA NVENC hevc encoder (codec hevc)
 V....D h264_qsv             H.264 (Intel Quick Sync Video) (codec h264)
 A....D aac                  AAC (Advanced Audio Coding)
";

    #[test]
    fn 只从编码器表里认出认识的硬件编码器() {
        let available = parse(FIXTURE);
        assert_eq!(
            available,
            vec!["h264_nvenc", "hevc_nvenc", "h264_qsv"],
            "顺序要按优先级来，且不能把 libx264 当成硬件"
        );
    }

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
        assert!(nvenc.windows(2).any(|pair| pair == ["-rc", "vbr"]));
        assert!(!nvenc.iter().any(|arg| arg == "-crf"));

        let amf = video_args("h264_amf", Quality::Small);
        assert!(amf.windows(2).any(|pair| pair == ["-b:v", "2M"]));
    }
}
