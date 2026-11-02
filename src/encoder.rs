//! 视频编码器探测：运行时问随程序分发的那份 ffmpeg，这台机器能不能硬件编码。
//!
//! 编译期不知道用户装的是 NVIDIA / Intel / AMD 还是 Apple 的显卡，只能启动后跑一次
//! `ffmpeg -encoders`。更关键的是「列出来的」不等于「能用的」——没插 NVIDIA 卡时
//! ffmpeg 仍会列出 `h264_nvenc`，真正初始化编码器时才失败，所以每个候选还要实编一帧。
//!
//! 结果缓存在进程内，裁切与格式转换共用；探测在后台线程跑，别卡界面。

use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

/// 编码器偏好：有显卡就用、强制软件、或强制硬件。裁切与格式转换共用。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EncoderMode {
    /// 探测到硬件编码器就用，否则回退 libx264/libx265。
    #[default]
    Auto,
    /// 始终用 libx264/libx265：画质与跨平台一致性最好。
    Software,
    /// 只用硬件编码器；探测不到时直接报错，不悄悄改回软件。
    Hardware,
}

impl EncoderMode {
    pub const ALL: [EncoderMode; 3] = [
        EncoderMode::Auto,
        EncoderMode::Software,
        EncoderMode::Hardware,
    ];

    pub fn label(self) -> &'static str {
        match self {
            EncoderMode::Auto => "自动",
            EncoderMode::Software => "软件",
            EncoderMode::Hardware => "硬件",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            EncoderMode::Auto => "有可用的显卡编码器就用，没有则用 x264 / x265",
            EncoderMode::Software => "始终用 x264 / x265：画质稳定，CPU 占用高",
            EncoderMode::Hardware => "只用显卡编码：速度快、CPU 占用低，画质随驱动变化",
        }
    }
}

/// 编码族。硬件编码器只按这两族区分，具体落到哪个厂商由探测结果定。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Family {
    H264,
    Hevc,
}

const H264_HARDWARE: &[&str] = &["h264_nvenc", "h264_qsv", "h264_amf", "h264_videotoolbox"];
const HEVC_HARDWARE: &[&str] = &["hevc_nvenc", "hevc_qsv", "hevc_amf", "hevc_videotoolbox"];

impl Family {
    /// 候选编码器，顺序就是优先级。
    ///
    /// 同一族里 NVIDIA 放最前：它跨三个平台都有，驱动成熟；QSV 次之；AMF 只在
    /// Windows；VideoToolbox 只在 macOS。真正可用的由探测结果决定。
    pub(crate) fn candidates(self) -> &'static [&'static str] {
        match self {
            Family::H264 => H264_HARDWARE,
            Family::Hevc => HEVC_HARDWARE,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Family::H264 => "H.264",
            Family::Hevc => "H.265",
        }
    }
}

/// 启动后台探测。放在应用启动时调用：探测要起好几个 ffmpeg 进程，
/// 不能在用户点「开始」的那一刻卡住界面。
pub(crate) fn warm_up() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        std::thread::spawn(|| {
            let names = probe_all();
            *AVAILABLE.lock().unwrap() = Some(names);
        });
    });
}

/// 探测结果快照：启动后由 [`warm_up`] 填好；空表示还在探测，或者这份 ffmpeg
/// 根本没有硬件编码器。
pub(crate) fn available() -> Vec<&'static str> {
    AVAILABLE.lock().unwrap().clone().unwrap_or_default()
}

/// 取这一族优先级最高的可用编码器。
pub(crate) fn pick(family: Family) -> Option<&'static str> {
    pick_from(family, &available())
}

/// 从给定的探测结果里挑，便于测试注入而不用真去跑 ffmpeg。
pub(crate) fn pick_from(family: Family, available: &[&'static str]) -> Option<&'static str> {
    family
        .candidates()
        .iter()
        .copied()
        .find(|name| available.contains(name))
}

/// 硬件编码器的码率参数（不含 `-c:v`）。
///
/// 硬件编码器一律用码率控制而不是 CRF：同一套 CRF 参数在 NVENC / QSV / AMF /
/// VideoToolbox 上含义不同，有的甚至不认 `-crf`，直接报错。
pub(crate) fn bitrate_args(encoder: &str, bitrate: &str) -> Vec<String> {
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
    args.iter().map(|arg| (*arg).to_string()).collect()
}

/// 这个名字是不是硬件编码器。软件编码器与它无关，调用方据此决定码控方式。
pub(crate) fn is_hardware(encoder: &str) -> bool {
    H264_HARDWARE.contains(&encoder) || HEVC_HARDWARE.contains(&encoder)
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

/// 编译进去 ≠ 这台机器能用。用 1 帧 64×64 黑画面试一次，几百毫秒内出结果。
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

/// 从 `-encoders` 的表格里挑出认识的硬件编码器。
fn parse(text: &str) -> Vec<&'static str> {
    H264_HARDWARE
        .iter()
        .chain(HEVC_HARDWARE)
        .copied()
        .filter(|name| {
            text.lines()
                .any(|line| line.split_whitespace().any(|token| token == *name))
        })
        .collect()
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
        let mut available = parse(FIXTURE);
        available.sort_unstable();
        assert_eq!(
            available,
            vec!["h264_nvenc", "h264_qsv", "hevc_nvenc"],
            "不能把 libx264 当成硬件；优先级由 pick_from 按 candidates 排，与这里的顺序无关"
        );
    }

    #[test]
    fn 按族挑选且不跨族误用() {
        let available = ["h264_qsv", "h264_nvenc"];
        assert_eq!(
            pick_from(Family::H264, &available),
            Some("h264_nvenc"),
            "同一族里 NVENC 优先"
        );
        assert_eq!(
            pick_from(Family::Hevc, &available),
            None,
            "只探测到 H.264 硬编时 HEVC 不能凑合"
        );
    }

    #[test]
    fn 每家硬件编码器的码控写法不同() {
        let nvenc = bitrate_args("hevc_nvenc", "5M");
        assert!(nvenc.windows(2).any(|pair| pair == ["-rc", "vbr"]));
        assert!(nvenc.windows(2).any(|pair| pair == ["-b:v", "5M"]));
        assert!(!nvenc.iter().any(|arg| arg == "-crf"));

        let qsv = bitrate_args("h264_qsv", "5M");
        assert!(qsv.windows(2).any(|pair| pair == ["-b:v", "5M"]));

        let amf = bitrate_args("h264_amf", "2M");
        assert!(amf.windows(2).any(|pair| pair == ["-rc", "vbr_peak"]));

        // VideoToolbox 只给码率，没有厂商特有的开关。
        assert_eq!(bitrate_args("h264_videotoolbox", "5M"), ["-b:v", "5M"]);
    }
}
