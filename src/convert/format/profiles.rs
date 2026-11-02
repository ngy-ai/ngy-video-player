//! 目标格式表本身：每个容器用哪套编码参数、面板上怎么说明。
//!
//! 视频档位一律 8bit `yuv420p`：转换的诉求是「别人也能播」，10bit 会让一部分
//! 硬解设备和浏览器放不出画面，而这些档位又默认走 x264/x265 这类最通用的编码器。
//!
//! H.264 / HEVC 档位不写死编码器：有可用的显卡编码器就用它（参数由
//! [`crate::encoder`] 按厂商生成），没有才落回 libx264 / libx265。

use super::TargetFormat;
use crate::encoder::{self, Family};

/// 硬件编码没有跨厂商通用的画质参数，只能给码率。取裁切那边的最高档：转换是
/// 用户主动重编码，画质不该比 CPU 档差太多，文件大小也还可以预期。
const HARDWARE_BITRATE: &str = "10M";

/// 一套编码参数。多个容器共用同一套时只写一次。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Profile {
    /// 面板上那句说明，跟着当前选中的格式显示。
    pub(super) hint: &'static str,
    /// 视频流怎么编码。
    pub(super) video: VideoProfile,
    /// 音频流参数；空表示这个容器不装音频（裸视频流）。
    pub(super) audio: &'static [&'static str],
    /// 字幕是否原样复制。
    pub(super) subtitles: bool,
}

/// 视频流怎么编码。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VideoProfile {
    /// 这个容器只装音频。
    None,
    /// H.264：能硬编就换显卡。
    H264 {
        /// 3gp / 3g2 这类老设备档位：保留 baseline，且不换硬编。
        baseline: bool,
        /// mp4 系把索引写到文件头，边下边播不用等整个文件。
        faststart: bool,
    },
    /// HEVC：同上，参数换成 x265 那一套。
    Hevc,
    /// 固定编码器与参数（VP9 / Theora / MPEG-2 …），没有硬件优选这回事。
    Fixed(&'static [&'static str]),
}

impl Profile {
    /// 这个容器还留不留视频轨。
    pub(super) fn keeps_video(self) -> bool {
        !matches!(self.video, VideoProfile::None)
    }

    /// 视频流参数（含 `-c:v`）。探测结果由调用方传进来：既方便测试固定场景，
    /// 也让一个批次里的每个文件用同一个编码器。
    pub(super) fn video_args_with(
        self,
        h264: Option<&'static str>,
        hevc: Option<&'static str>,
    ) -> Vec<String> {
        match self.video {
            VideoProfile::None => Vec::new(),
            VideoProfile::Fixed(args) => args.iter().map(|arg| (*arg).to_string()).collect(),
            VideoProfile::H264 {
                baseline,
                faststart,
            } => family_args(Family::H264, h264, baseline, faststart),
            VideoProfile::Hevc => family_args(Family::Hevc, hevc, false, false),
        }
    }
}

/// H.264 / HEVC 的参数：能硬编就硬编，否则用 CPU 那套。
fn family_args(
    family: Family,
    hardware: Option<&'static str>,
    baseline: bool,
    faststart: bool,
) -> Vec<String> {
    let mut args = Vec::new();
    // 老设备档位不换硬编：各家驱动对 baseline 的支持不一致，这一档的卖点就是
    // 「插到任何机器上都能放」，宁可慢也别赌驱动。
    match hardware {
        Some(hardware) if !baseline => {
            args.push("-c:v".to_string());
            args.push(hardware.to_string());
            args.extend(encoder::bitrate_args(hardware, HARDWARE_BITRATE));
            // 多数硬件编码器只吃 8bit 4:2:0，不指定会在初始化阶段失败。
            args.extend(["-pix_fmt", "yuv420p"].map(str::to_string));
        }
        _ => {
            args.push("-c:v".to_string());
            args.push(software_encoder(family).to_string());
            args.extend(
                [
                    "-preset",
                    "medium",
                    "-crf",
                    crf(family),
                    "-pix_fmt",
                    "yuv420p",
                ]
                .map(str::to_string),
            );
        }
    }
    if baseline {
        args.extend(["-profile:v", "baseline"].map(str::to_string));
    }
    // 索引位置与用哪个编码器无关：硬编照旧。
    if faststart {
        args.extend(["-movflags", "+faststart"].map(str::to_string));
    }
    args
}

fn software_encoder(family: Family) -> &'static str {
    match family {
        Family::H264 => "libx264",
        Family::Hevc => "libx265",
    }
}

fn crf(family: Family) -> &'static str {
    match family {
        Family::H264 => "20",
        Family::Hevc => "26",
    }
}

/// 表项的简写，省掉每条都写一遍结构体名。
pub(super) const fn target(
    extension: &'static str,
    muxer: &'static str,
    profile: Profile,
) -> TargetFormat {
    TargetFormat {
        extension,
        muxer,
        profile,
    }
}

// -- 视频档位 --------------------------------------------------------------

const AAC192: &[&str] = &["-c:a", "aac", "-b:a", "192k"];
const MP3_192: &[&str] = &["-c:a", "libmp3lame", "-b:a", "192k"];
const MPEG2: &[&str] = &["-c:v", "mpeg2video", "-q:v", "4"];

const H264_AAC: Profile = Profile {
    hint: "H.264 + AAC",
    video: VideoProfile::H264 {
        baseline: false,
        faststart: false,
    },
    audio: AAC192,
    subtitles: false,
};
const H264_AAC_FASTSTART: Profile = Profile {
    hint: "H.264 + AAC · 兼容性最好",
    video: VideoProfile::H264 {
        baseline: false,
        faststart: true,
    },
    audio: AAC192,
    subtitles: false,
};
const H264_AAC_SUBTITLES: Profile = Profile {
    hint: "H.264 + AAC · 保留字幕",
    video: VideoProfile::H264 {
        baseline: false,
        faststart: false,
    },
    audio: AAC192,
    subtitles: true,
};
const H264_MP3: Profile = Profile {
    hint: "H.264 + MP3",
    video: VideoProfile::H264 {
        baseline: false,
        faststart: false,
    },
    audio: MP3_192,
    subtitles: false,
};
/// 老设备（早期手机、部分车载）只认 baseline。
const H264_BASELINE: Profile = Profile {
    hint: "H.264 基线 + AAC · 老设备",
    video: VideoProfile::H264 {
        baseline: true,
        faststart: false,
    },
    audio: AAC192,
    subtitles: false,
};
const VP9_OPUS: Profile = Profile {
    hint: "VP9 + Opus · 编码慢",
    video: VideoProfile::Fixed(&[
        "-c:v",
        "libvpx-vp9",
        "-crf",
        "32",
        "-b:v",
        "0",
        "-row-mt",
        "1",
    ]),
    audio: &["-c:a", "libopus", "-b:a", "128k"],
    subtitles: false,
};
const WMV2_WMA: Profile = Profile {
    hint: "WMV2 + WMA",
    video: VideoProfile::Fixed(&["-c:v", "wmv2", "-b:v", "2000k"]),
    audio: &["-c:a", "wmav2", "-b:a", "192k"],
    subtitles: false,
};
const MPEG2_MP2: Profile = Profile {
    hint: "MPEG-2 + MP2",
    video: VideoProfile::Fixed(MPEG2),
    audio: &["-c:a", "mp2", "-b:a", "192k"],
    subtitles: false,
};
const MPEG2_ONLY: Profile = Profile {
    hint: "MPEG-2 裸流 · 无音轨",
    video: VideoProfile::Fixed(MPEG2),
    audio: &[],
    subtitles: false,
};
const MPEG1_ONLY: Profile = Profile {
    hint: "MPEG-1 裸流 · 无音轨",
    video: VideoProfile::Fixed(&["-c:v", "mpeg1video", "-q:v", "4"]),
    audio: &[],
    subtitles: false,
};
/// MXF 只认 48kHz 立体声，采样率不统一会被封装器直接拒绝。
const MPEG2_PCM: Profile = Profile {
    hint: "MPEG-2 + PCM · 广播用",
    video: VideoProfile::Fixed(MPEG2),
    audio: &["-c:a", "pcm_s16le", "-ar", "48000", "-ac", "2"],
    subtitles: false,
};
const THEORA_VORBIS: Profile = Profile {
    hint: "Theora + Vorbis",
    video: VideoProfile::Fixed(&["-c:v", "libtheora", "-q:v", "7"]),
    audio: &["-c:a", "libvorbis", "-q:a", "4"],
    subtitles: false,
};
const RV20_AC3: Profile = Profile {
    hint: "RealVideo 2 + AC-3",
    video: VideoProfile::Fixed(&["-c:v", "rv20", "-b:v", "1500k"]),
    audio: &["-c:a", "ac3", "-b:a", "192k"],
    subtitles: false,
};
/// DV 固定 720×576/25p：不做等比缩放加黑边，非 4:3 的源会被拉变形。
const DV_PCM: Profile = Profile {
    hint: "DV + PCM · 固定 720×576",
    video: VideoProfile::Fixed(&[
        "-c:v",
        "dvvideo",
        "-pix_fmt",
        "yuv420p",
        "-vf",
        "scale=720:576:force_original_aspect_ratio=decrease,pad=720:576:-1:-1",
        "-r",
        "25",
    ]),
    audio: &["-c:a", "pcm_s16le", "-ar", "48000", "-ac", "2"],
    subtitles: false,
};
const Y4M: Profile = Profile {
    hint: "未压缩帧 · 无音轨",
    video: VideoProfile::Fixed(&["-c:v", "wrapped_avframe", "-pix_fmt", "yuv420p"]),
    audio: &[],
    subtitles: false,
};
/// RoQ 要求宽高是 16 的倍数，往下取整最稳妥（不放大，也不留黑边）。
const ROQ: Profile = Profile {
    hint: "RoQ · 尺寸按 16 对齐",
    video: VideoProfile::Fixed(&[
        "-c:v",
        "roqvideo",
        "-q:v",
        "4",
        "-vf",
        "scale=trunc(iw/16)*16:trunc(ih/16)*16",
    ]),
    audio: &[],
    subtitles: false,
};
const H264_RAW: Profile = Profile {
    hint: "H.264 裸流 · 无音轨",
    video: VideoProfile::H264 {
        baseline: false,
        faststart: false,
    },
    audio: &[],
    subtitles: false,
};
const H265_RAW: Profile = Profile {
    hint: "HEVC 裸流 · 无音轨",
    video: VideoProfile::Hevc,
    audio: &[],
    subtitles: false,
};
const AV1_RAW: Profile = Profile {
    hint: "AV1 裸流 · 无音轨",
    video: VideoProfile::Fixed(&[
        "-c:v",
        "libaom-av1",
        "-cpu-used",
        "8",
        "-crf",
        "35",
        "-row-mt",
        "1",
        "-pix_fmt",
        "yuv420p",
    ]),
    audio: &[],
    subtitles: false,
};

// -- 音频档位 --------------------------------------------------------------

const AUDIO_MP3: Profile = Profile {
    hint: "MP3 320k · 通用音频",
    video: VideoProfile::None,
    audio: &["-c:a", "libmp3lame", "-b:a", "320k"],
    subtitles: false,
};
const AUDIO_MP2: Profile = Profile {
    hint: "MPEG Layer II 192k",
    video: VideoProfile::None,
    audio: &["-c:a", "mp2", "-b:a", "192k"],
    subtitles: false,
};
const AUDIO_AAC: Profile = Profile {
    hint: "AAC 192k",
    video: VideoProfile::None,
    audio: AAC192,
    subtitles: false,
};
const AUDIO_AAC_M4A: Profile = Profile {
    hint: "AAC 256k · 苹果设备",
    video: VideoProfile::None,
    audio: &["-c:a", "aac", "-b:a", "256k"],
    subtitles: false,
};
const AUDIO_ALAC: Profile = Profile {
    hint: "ALAC 无损",
    video: VideoProfile::None,
    audio: &["-c:a", "alac"],
    subtitles: false,
};
const AUDIO_FLAC: Profile = Profile {
    hint: "FLAC 无损压缩",
    video: VideoProfile::None,
    audio: &["-c:a", "flac"],
    subtitles: false,
};
const AUDIO_PCM_LE: Profile = Profile {
    hint: "PCM 未压缩",
    video: VideoProfile::None,
    audio: &["-c:a", "pcm_s16le"],
    subtitles: false,
};
const AUDIO_PCM_BE: Profile = Profile {
    hint: "PCM 未压缩",
    video: VideoProfile::None,
    audio: &["-c:a", "pcm_s16be"],
    subtitles: false,
};
const AUDIO_VORBIS: Profile = Profile {
    hint: "Vorbis 质量 5",
    video: VideoProfile::None,
    audio: &["-c:a", "libvorbis", "-q:a", "5"],
    subtitles: false,
};
const AUDIO_OPUS: Profile = Profile {
    hint: "Opus 128k",
    video: VideoProfile::None,
    audio: &["-c:a", "libopus", "-b:a", "128k"],
    subtitles: false,
};
const AUDIO_SPEEX: Profile = Profile {
    hint: "Speex 语音",
    video: VideoProfile::None,
    audio: &["-c:a", "libspeex"],
    subtitles: false,
};
const AUDIO_WMA: Profile = Profile {
    hint: "WMA 192k",
    video: VideoProfile::None,
    audio: &["-c:a", "wmav2", "-b:a", "192k"],
    subtitles: false,
};
const AUDIO_AC3: Profile = Profile {
    hint: "AC-3 192k",
    video: VideoProfile::None,
    audio: &["-c:a", "ac3", "-b:a", "192k"],
    subtitles: false,
};
const AUDIO_EAC3: Profile = Profile {
    hint: "E-AC-3 192k",
    video: VideoProfile::None,
    audio: &["-c:a", "eac3", "-b:a", "192k"],
    subtitles: false,
};
/// dca 编码器在 ffmpeg 里仍是实验性的，必须带 `-strict -2`。
const AUDIO_DTS: Profile = Profile {
    hint: "DTS 768k",
    video: VideoProfile::None,
    audio: &["-c:a", "dca", "-strict", "-2", "-b:a", "768k"],
    subtitles: false,
};
const AUDIO_AMR: Profile = Profile {
    hint: "AMR-NB 12.2k · 语音",
    video: VideoProfile::None,
    audio: &[
        "-c:a",
        "libopencore_amrnb",
        "-b:a",
        "12.2k",
        "-ar",
        "8000",
        "-ac",
        "1",
    ],
    subtitles: false,
};
const AUDIO_WAVPACK: Profile = Profile {
    hint: "WavPack 无损",
    video: VideoProfile::None,
    audio: &["-c:a", "wavpack"],
    subtitles: false,
};

impl TargetFormat {
    /// 全部目标格式。顺序就是面板上的顺序：常用的排前面。
    ///
    /// 表里每一条都在 ffmpeg 6 上实转过一遍（含 `-f`），不是照扩展名猜的。
    pub const ALL: &'static [TargetFormat] = &[
        // 视频：先给最常用的几个，再按容器家族铺开。
        target("mp4", "mp4", H264_AAC_FASTSTART),
        target("mkv", "matroska", H264_AAC_SUBTITLES),
        target("mov", "mov", H264_AAC_FASTSTART),
        target("webm", "webm", VP9_OPUS),
        target("avi", "avi", H264_MP3),
        target("ts", "mpegts", H264_AAC),
        target("flv", "flv", H264_AAC),
        target("wmv", "asf", WMV2_WMA),
        target("f4v", "f4v", H264_AAC),
        target("m4v", "mp4", H264_AAC_FASTSTART),
        target("m2ts", "mpegts", H264_AAC),
        target("mpg", "mpeg", MPEG2_MP2),
        target("vob", "vob", MPEG2_MP2),
        target("mpeg", "mpeg", MPEG2_MP2),
        target("mpv", "mpeg2video", MPEG2_ONLY),
        target("m1v", "mpeg1video", MPEG1_ONLY),
        target("m2v", "mpeg2video", MPEG2_ONLY),
        target("mpe", "mpeg", MPEG2_MP2),
        target("mts", "mpegts", H264_AAC),
        target("m2t", "mpegts", H264_AAC),
        target("tp", "mpegts", H264_AAC),
        target("trp", "mpegts", H264_AAC),
        target("3gp", "3gp", H264_BASELINE),
        target("3g2", "3g2", H264_BASELINE),
        target("3gpp", "3gp", H264_BASELINE),
        target("asf", "asf", WMV2_WMA),
        target("nut", "nut", H264_AAC),
        target("mxf", "mxf", MPEG2_PCM),
        target("wtv", "wtv", H264_AAC),
        target("ogv", "ogv", THEORA_VORBIS),
        target("ogm", "ogg", THEORA_VORBIS),
        target("rm", "rm", RV20_AC3),
        target("rmvb", "rm", RV20_AC3),
        target("dv", "dv", DV_PCM),
        target("y4m", "yuv4mpegpipe", Y4M),
        target("h264", "h264", H264_RAW),
        target("h265", "hevc", H265_RAW),
        target("hevc", "hevc", H265_RAW),
        target("av1", "obu", AV1_RAW),
        target("roq", "roq", ROQ),
        // 音频。
        target("mp3", "mp3", AUDIO_MP3),
        target("m4a", "ipod", AUDIO_AAC_M4A),
        target("aac", "adts", AUDIO_AAC),
        target("flac", "flac", AUDIO_FLAC),
        target("wav", "wav", AUDIO_PCM_LE),
        target("wave", "wav", AUDIO_PCM_LE),
        target("ogg", "ogg", AUDIO_VORBIS),
        target("oga", "oga", AUDIO_VORBIS),
        target("ogx", "ogg", AUDIO_VORBIS),
        target("opus", "opus", AUDIO_OPUS),
        target("wma", "asf", AUDIO_WMA),
        target("ac3", "ac3", AUDIO_AC3),
        target("eac3", "eac3", AUDIO_EAC3),
        target("dts", "dts", AUDIO_DTS),
        target("mka", "matroska", AUDIO_FLAC),
        target("aiff", "aiff", AUDIO_PCM_BE),
        target("aif", "aiff", AUDIO_PCM_BE),
        target("aifc", "aiff", AUDIO_PCM_BE),
        target("au", "au", AUDIO_PCM_BE),
        target("snd", "au", AUDIO_PCM_BE),
        target("alac", "ipod", AUDIO_ALAC),
        target("caf", "caf", AUDIO_PCM_LE),
        target("wv", "wv", AUDIO_WAVPACK),
        target("mp2", "mp2", AUDIO_MP2),
        target("mpa", "mp2", AUDIO_MP2),
        target("spx", "spx", AUDIO_SPEEX),
        target("amr", "amr", AUDIO_AMR),
    ];
}
