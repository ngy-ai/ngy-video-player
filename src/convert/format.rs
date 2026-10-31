//! 目标格式：容器与编码的固定搭配。
//!
//! 刻意不提供「只换容器、流复制」的档位：源文件里的编码五花八门（HEVC 10bit、
//! AV1、PGS 字幕），流复制常常能写进新容器却播不出来，用户看到的是「转换成功
//! 但画面坏了」。一律重编码，代价是慢，但结果可预期。

/// 目标属于哪一类：视频容器还是纯音频。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetKind {
    Video,
    Audio,
}

impl TargetKind {
    pub fn label(self) -> &'static str {
        match self {
            TargetKind::Video => "视频格式",
            TargetKind::Audio => "音频格式",
        }
    }
}

/// 可以转成的格式。顺序即面板上的顺序：先视频、后音频。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetFormat {
    Mp4,
    Mkv,
    Mov,
    Webm,
    Mp3,
    M4a,
    Flac,
    Wav,
}

impl TargetFormat {
    pub const ALL: [TargetFormat; 8] = [
        TargetFormat::Mp4,
        TargetFormat::Mkv,
        TargetFormat::Mov,
        TargetFormat::Webm,
        TargetFormat::Mp3,
        TargetFormat::M4a,
        TargetFormat::Flac,
        TargetFormat::Wav,
    ];

    /// 卡片上的大类标题。
    pub fn kind(self) -> TargetKind {
        match self {
            TargetFormat::Mp4 | TargetFormat::Mkv | TargetFormat::Mov | TargetFormat::Webm => {
                TargetKind::Video
            }
            TargetFormat::Mp3 | TargetFormat::M4a | TargetFormat::Flac | TargetFormat::Wav => {
                TargetKind::Audio
            }
        }
    }

    /// 目标容器里是否还留视频轨。纯音频目标会把画面整个丢掉。
    pub fn keeps_video(self) -> bool {
        self.kind() == TargetKind::Video
    }

    pub fn label(self) -> &'static str {
        match self {
            TargetFormat::Mp4 => "MP4",
            TargetFormat::Mkv => "MKV",
            TargetFormat::Mov => "MOV",
            TargetFormat::Webm => "WebM",
            TargetFormat::Mp3 => "MP3",
            TargetFormat::M4a => "M4A",
            TargetFormat::Flac => "FLAC",
            TargetFormat::Wav => "WAV",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            TargetFormat::Mp4 => "mp4",
            TargetFormat::Mkv => "mkv",
            TargetFormat::Mov => "mov",
            TargetFormat::Webm => "webm",
            TargetFormat::Mp3 => "mp3",
            TargetFormat::M4a => "m4a",
            TargetFormat::Flac => "flac",
            TargetFormat::Wav => "wav",
        }
    }

    /// 卡片上的小字：选之前就该知道的代价。
    pub fn hint(self) -> &'static str {
        match self {
            TargetFormat::Mp4 => "H.264 + AAC · 兼容性最好",
            TargetFormat::Mkv => "H.264 + AAC · 保留字幕",
            TargetFormat::Mov => "H.264 + AAC · 苹果设备",
            TargetFormat::Webm => "VP9 + Opus · 编码较慢",
            TargetFormat::Mp3 => "MP3 320k · 通用音频",
            TargetFormat::M4a => "AAC 256k · 苹果设备",
            TargetFormat::Flac => "无损压缩",
            TargetFormat::Wav => "未压缩 · 体积大",
        }
    }

    /// 视频轨与音频轨的编码参数。
    ///
    /// 用 crf 而不是固定码率：教学视频里大量静止画面，按质量压比按码率压
    /// 更省空间，也不会在复杂场景里糊掉。
    pub(crate) fn codec_args(self) -> &'static [&'static str] {
        match self {
            TargetFormat::Mp4 | TargetFormat::Mov => &[
                "-c:v",
                "libx264",
                "-preset",
                "medium",
                "-crf",
                "20",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
                "-movflags",
                "+faststart",
            ],
            TargetFormat::Mkv => &[
                "-c:v", "libx264", "-preset", "medium", "-crf", "20", "-pix_fmt", "yuv420p",
                "-c:a", "aac", "-b:a", "192k",
            ],
            TargetFormat::Webm => &[
                "-c:v",
                "libvpx-vp9",
                "-crf",
                "32",
                "-b:v",
                "0",
                "-row-mt",
                "1",
                "-c:a",
                "libopus",
                "-b:a",
                "128k",
            ],
            TargetFormat::Mp3 => &["-c:a", "libmp3lame", "-b:a", "320k"],
            TargetFormat::M4a => &["-c:a", "aac", "-b:a", "256k"],
            TargetFormat::Flac => &["-c:a", "flac"],
            TargetFormat::Wav => &["-c:a", "pcm_s16le"],
        }
    }

    /// 字幕怎么处理。
    ///
    /// 只有 mkv 能原样装下任意字幕（ASS、PGS 都行），其它容器要么不支持图像字幕，
    /// 要么得逐条转码——转失败会让整个文件报废。装不下就显式丢掉：面板上写明了
    /// 「MP4 / MOV / WebM 不带字幕」，用户不会以为字幕悄悄丢了是 bug。
    pub(crate) fn subtitle_args(self) -> &'static [&'static str] {
        match self {
            TargetFormat::Mkv => &["-c:s", "copy"],
            _ => &["-sn"],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 每个格式都有扩展名与说明() {
        for format in TargetFormat::ALL {
            assert!(!format.label().is_empty(), "{format:?}");
            assert!(!format.hint().is_empty(), "{format:?}");
            let extension = format.extension();
            assert!(
                extension
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit()),
                "{format:?} 的扩展名应当是小写：{extension}"
            );
        }
    }

    #[test]
    fn 扩展名互不重复() {
        let mut extensions: Vec<&str> = TargetFormat::ALL
            .iter()
            .map(|format| format.extension())
            .collect();
        extensions.sort_unstable();
        let total = extensions.len();
        extensions.dedup();
        assert_eq!(extensions.len(), total, "同一时刻只能有一个格式用这个后缀");
    }

    #[test]
    fn 视频与音频各占一半() {
        let video = TargetFormat::ALL
            .iter()
            .filter(|format| format.keeps_video())
            .count();
        assert_eq!(video, 4);
        assert_eq!(TargetFormat::ALL.len() - video, 4);
    }

    #[test]
    fn 只有_mkv_保留字幕() {
        assert_eq!(TargetFormat::Mkv.subtitle_args(), &["-c:s", "copy"]);
        for format in TargetFormat::ALL {
            if format != TargetFormat::Mkv {
                assert_eq!(format.subtitle_args(), &["-sn"], "{format:?}");
            }
        }
    }
}
