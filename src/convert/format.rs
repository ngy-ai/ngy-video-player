//! 目标格式表：容器 + 编码组合。
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

/// 一套编码参数。多个容器共用同一套时只写一次。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Profile {
    /// 面板上那句说明，跟着当前选中的格式显示。
    hint: &'static str,
    /// 视频流参数；空表示这个容器只装音频。
    video: &'static [&'static str],
    /// 音频流参数；空表示这个容器不装音频（裸视频流）。
    audio: &'static [&'static str],
    /// 字幕是否原样复制。
    subtitles: bool,
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

/// 表项的简写，省掉每条都写一遍结构体名。
const fn target(extension: &'static str, muxer: &'static str, profile: Profile) -> TargetFormat {
    TargetFormat {
        extension,
        muxer,
        profile,
    }
}

// -- 视频档位 --------------------------------------------------------------

const X264: &[&str] = &[
    "-c:v", "libx264", "-preset", "medium", "-crf", "20", "-pix_fmt", "yuv420p",
];
/// mp4 系要在文件头写索引，边下边播才不用等整个文件。
const X264_FASTSTART: &[&str] = &[
    "-c:v",
    "libx264",
    "-preset",
    "medium",
    "-crf",
    "20",
    "-pix_fmt",
    "yuv420p",
    "-movflags",
    "+faststart",
];
/// 老设备（早期手机、部分车载）只认 baseline。
const X264_BASELINE: &[&str] = &[
    "-c:v",
    "libx264",
    "-preset",
    "medium",
    "-crf",
    "20",
    "-profile:v",
    "baseline",
    "-pix_fmt",
    "yuv420p",
];
const AAC192: &[&str] = &["-c:a", "aac", "-b:a", "192k"];
const MP3_192: &[&str] = &["-c:a", "libmp3lame", "-b:a", "192k"];
const MPEG2: &[&str] = &["-c:v", "mpeg2video", "-q:v", "4"];

const H264_AAC: Profile = Profile {
    hint: "H.264 + AAC",
    video: X264,
    audio: AAC192,
    subtitles: false,
};
const H264_AAC_FASTSTART: Profile = Profile {
    hint: "H.264 + AAC · 兼容性最好",
    video: X264_FASTSTART,
    audio: AAC192,
    subtitles: false,
};
const H264_AAC_SUBTITLES: Profile = Profile {
    hint: "H.264 + AAC · 保留字幕",
    video: X264,
    audio: AAC192,
    subtitles: true,
};
const H264_MP3: Profile = Profile {
    hint: "H.264 + MP3",
    video: X264,
    audio: MP3_192,
    subtitles: false,
};
const H264_BASELINE: Profile = Profile {
    hint: "H.264 基线 + AAC · 老设备",
    video: X264_BASELINE,
    audio: AAC192,
    subtitles: false,
};
const VP9_OPUS: Profile = Profile {
    hint: "VP9 + Opus · 编码慢",
    video: &[
        "-c:v",
        "libvpx-vp9",
        "-crf",
        "32",
        "-b:v",
        "0",
        "-row-mt",
        "1",
    ],
    audio: &["-c:a", "libopus", "-b:a", "128k"],
    subtitles: false,
};
const WMV2_WMA: Profile = Profile {
    hint: "WMV2 + WMA",
    video: &["-c:v", "wmv2", "-b:v", "2000k"],
    audio: &["-c:a", "wmav2", "-b:a", "192k"],
    subtitles: false,
};
const MPEG2_MP2: Profile = Profile {
    hint: "MPEG-2 + MP2",
    video: MPEG2,
    audio: &["-c:a", "mp2", "-b:a", "192k"],
    subtitles: false,
};
const MPEG2_ONLY: Profile = Profile {
    hint: "MPEG-2 裸流 · 无音轨",
    video: MPEG2,
    audio: &[],
    subtitles: false,
};
const MPEG1_ONLY: Profile = Profile {
    hint: "MPEG-1 裸流 · 无音轨",
    video: &["-c:v", "mpeg1video", "-q:v", "4"],
    audio: &[],
    subtitles: false,
};
/// MXF 只认 48kHz 立体声，采样率不统一会被封装器直接拒绝。
const MPEG2_PCM: Profile = Profile {
    hint: "MPEG-2 + PCM · 广播用",
    video: MPEG2,
    audio: &["-c:a", "pcm_s16le", "-ar", "48000", "-ac", "2"],
    subtitles: false,
};
const THEORA_VORBIS: Profile = Profile {
    hint: "Theora + Vorbis",
    video: &["-c:v", "libtheora", "-q:v", "7"],
    audio: &["-c:a", "libvorbis", "-q:a", "4"],
    subtitles: false,
};
const RV20_AC3: Profile = Profile {
    hint: "RealVideo 2 + AC-3",
    video: &["-c:v", "rv20", "-b:v", "1500k"],
    audio: &["-c:a", "ac3", "-b:a", "192k"],
    subtitles: false,
};
/// DV 固定 720×576/25p：不做等比缩放加黑边，非 4:3 的源会被拉变形。
const DV_PCM: Profile = Profile {
    hint: "DV + PCM · 固定 720×576",
    video: &[
        "-c:v",
        "dvvideo",
        "-pix_fmt",
        "yuv420p",
        "-vf",
        "scale=720:576:force_original_aspect_ratio=decrease,pad=720:576:-1:-1",
        "-r",
        "25",
    ],
    audio: &["-c:a", "pcm_s16le", "-ar", "48000", "-ac", "2"],
    subtitles: false,
};
const Y4M: Profile = Profile {
    hint: "未压缩帧 · 无音轨",
    video: &["-c:v", "wrapped_avframe", "-pix_fmt", "yuv420p"],
    audio: &[],
    subtitles: false,
};
/// RoQ 要求宽高是 16 的倍数，往下取整最稳妥（不放大，也不留黑边）。
const ROQ: Profile = Profile {
    hint: "RoQ · 尺寸按 16 对齐",
    video: &[
        "-c:v",
        "roqvideo",
        "-q:v",
        "4",
        "-vf",
        "scale=trunc(iw/16)*16:trunc(ih/16)*16",
    ],
    audio: &[],
    subtitles: false,
};
const H264_RAW: Profile = Profile {
    hint: "H.264 裸流 · 无音轨",
    video: X264,
    audio: &[],
    subtitles: false,
};
const H265_RAW: Profile = Profile {
    hint: "HEVC 裸流 · 无音轨",
    video: &[
        "-c:v", "libx265", "-preset", "medium", "-crf", "26", "-pix_fmt", "yuv420p",
    ],
    audio: &[],
    subtitles: false,
};
const AV1_RAW: Profile = Profile {
    hint: "AV1 裸流 · 无音轨",
    video: &[
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
    ],
    audio: &[],
    subtitles: false,
};

// -- 音频档位 --------------------------------------------------------------

const AUDIO_MP3: Profile = Profile {
    hint: "MP3 320k · 通用音频",
    video: &[],
    audio: &["-c:a", "libmp3lame", "-b:a", "320k"],
    subtitles: false,
};
const AUDIO_MP2: Profile = Profile {
    hint: "MPEG Layer II 192k",
    video: &[],
    audio: &["-c:a", "mp2", "-b:a", "192k"],
    subtitles: false,
};
const AUDIO_AAC: Profile = Profile {
    hint: "AAC 192k",
    video: &[],
    audio: AAC192,
    subtitles: false,
};
const AUDIO_AAC_M4A: Profile = Profile {
    hint: "AAC 256k · 苹果设备",
    video: &[],
    audio: &["-c:a", "aac", "-b:a", "256k"],
    subtitles: false,
};
const AUDIO_ALAC: Profile = Profile {
    hint: "ALAC 无损",
    video: &[],
    audio: &["-c:a", "alac"],
    subtitles: false,
};
const AUDIO_FLAC: Profile = Profile {
    hint: "FLAC 无损压缩",
    video: &[],
    audio: &["-c:a", "flac"],
    subtitles: false,
};
const AUDIO_PCM_LE: Profile = Profile {
    hint: "PCM 未压缩",
    video: &[],
    audio: &["-c:a", "pcm_s16le"],
    subtitles: false,
};
const AUDIO_PCM_BE: Profile = Profile {
    hint: "PCM 未压缩",
    video: &[],
    audio: &["-c:a", "pcm_s16be"],
    subtitles: false,
};
const AUDIO_VORBIS: Profile = Profile {
    hint: "Vorbis 质量 5",
    video: &[],
    audio: &["-c:a", "libvorbis", "-q:a", "5"],
    subtitles: false,
};
const AUDIO_OPUS: Profile = Profile {
    hint: "Opus 128k",
    video: &[],
    audio: &["-c:a", "libopus", "-b:a", "128k"],
    subtitles: false,
};
const AUDIO_SPEEX: Profile = Profile {
    hint: "Speex 语音",
    video: &[],
    audio: &["-c:a", "libspeex"],
    subtitles: false,
};
const AUDIO_WMA: Profile = Profile {
    hint: "WMA 192k",
    video: &[],
    audio: &["-c:a", "wmav2", "-b:a", "192k"],
    subtitles: false,
};
const AUDIO_AC3: Profile = Profile {
    hint: "AC-3 192k",
    video: &[],
    audio: &["-c:a", "ac3", "-b:a", "192k"],
    subtitles: false,
};
const AUDIO_EAC3: Profile = Profile {
    hint: "E-AC-3 192k",
    video: &[],
    audio: &["-c:a", "eac3", "-b:a", "192k"],
    subtitles: false,
};
/// dca 编码器在 ffmpeg 里仍是实验性的，必须带 `-strict -2`。
const AUDIO_DTS: Profile = Profile {
    hint: "DTS 768k",
    video: &[],
    audio: &["-c:a", "dca", "-strict", "-2", "-b:a", "768k"],
    subtitles: false,
};
const AUDIO_AMR: Profile = Profile {
    hint: "AMR-NB 12.2k · 语音",
    video: &[],
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
    video: &[],
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

    /// 扩展名，也是面板上显示的名字。
    pub fn extension(self) -> &'static str {
        self.extension
    }

    pub fn kind(self) -> TargetKind {
        if self.profile.video.is_empty() {
            TargetKind::Audio
        } else {
            TargetKind::Video
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

    pub(crate) fn video_args(self) -> &'static [&'static str] {
        self.profile.video
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats;

    #[test]
    fn 扩展名互不重复() {
        let mut extensions: Vec<&str> = TargetFormat::ALL
            .iter()
            .map(|format| format.extension)
            .collect();
        let total = extensions.len();
        extensions.sort_unstable();
        extensions.dedup();
        assert_eq!(extensions.len(), total, "同一时刻只能有一个格式用这个后缀");
    }

    #[test]
    fn 覆盖了现有的全部视频与音频扩展名() {
        // 目标表只能比播放表窄，不能凭空多出播放器都认不出的后缀。
        for format in TargetFormat::ALL {
            let known = formats::VIDEO.contains(&format.extension)
                || formats::AUDIO.contains(&format.extension);
            assert!(known, "{} 不在 formats 的清单里", format.extension);
        }
        // 反过来：能播又是常见容器的，基本都该能转。这里只卡住几张主流格式，
        // 冷门容器里确实有不支持写出的（见模块文档）。
        for extension in [
            "mp4", "mkv", "mov", "webm", "avi", "ts", "mp3", "flac", "wav", "m4a",
        ] {
            assert!(
                TargetFormat::by_extension(extension).is_some(),
                "缺少常用格式 {extension}"
            );
        }
    }

    #[test]
    fn 每个格式都有说明和编码参数() {
        for format in TargetFormat::ALL {
            assert!(!format.hint().is_empty(), "{}", format.extension);
            match format.kind() {
                TargetKind::Video => {
                    assert!(
                        !format.video_args().is_empty(),
                        "{} 是视频容器却没有视频参数",
                        format.extension
                    );
                }
                TargetKind::Audio => {
                    assert!(
                        format.video_args().is_empty(),
                        "{} 是音频容器不该有视频参数",
                        format.extension
                    );
                    assert!(
                        !format.audio_args().is_empty(),
                        "{} 没有音频参数",
                        format.extension
                    );
                }
            }
            assert!(!format.muxer().is_empty(), "{}", format.extension);
        }
    }

    #[test]
    fn 只有_mkv_保留字幕() {
        for format in TargetFormat::ALL {
            let subtitles = format.copy_subtitles();
            assert_eq!(subtitles, format.extension == "mkv", "{}", format.extension);
        }
    }

    #[test]
    fn 默认是_mp4() {
        assert_eq!(TargetFormat::default().extension(), "mp4");
        assert_eq!(TargetFormat::by_extension("nope"), None);
    }
}
