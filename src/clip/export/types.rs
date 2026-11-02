//! 导出参数里的枚举：剪切方式、输出格式、编码、画质、像素格式。
//!
//! 全是「几选一」的小类型，一律配 `ALL` + `label` + `hint` 三件套，面板照着铺
//! 按钮就行。集中在一个文件是因为它们只被 `ExportOptions` 与界面引用，
//! 和「怎么拼 ffmpeg 参数」没有关系。

/// 剪切方式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExportMode {
    /// 逐帧重编码：切点精确到帧，兼容性最好。
    #[default]
    Exact,
    /// 流复制：不动画面与音频，速度快，切点对齐到关键帧。
    Copy,
}

impl ExportMode {
    pub const ALL: [ExportMode; 2] = [ExportMode::Exact, ExportMode::Copy];

    pub fn label(self) -> &'static str {
        match self {
            ExportMode::Exact => "精确重编码",
            ExportMode::Copy => "无损流复制",
        }
    }

    /// 面板上给这一档的一句话说明。
    pub fn hint(self) -> &'static str {
        match self {
            ExportMode::Exact => "切点精确到帧，H.264/AAC 兼容性最好，速度取决于片长",
            ExportMode::Copy => "不重新编码，几秒完成；起点会吸附到关键帧，滤镜与响度不可用",
        }
    }
}

/// 导出成单文件还是 HLS 切片。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutputFormat {
    /// 一个 mkv 装下全部轨道。
    #[default]
    Mkv,
    /// m3u8 + 一串 .ts 分片，供网页 / 流媒体服务器直接使用。
    Hls,
}

impl OutputFormat {
    pub const ALL: [OutputFormat; 2] = [OutputFormat::Mkv, OutputFormat::Hls];

    pub fn label(self) -> &'static str {
        match self {
            OutputFormat::Mkv => "单文件 mkv",
            OutputFormat::Hls => "HLS 切片",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            OutputFormat::Mkv => "一个文件装下画面、音轨、字幕与章节",
            OutputFormat::Hls => "输出 m3u8 与 .ts 分片；只保留画面与音轨，字幕/章节不写入",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            OutputFormat::Mkv => "mkv",
            OutputFormat::Hls => "m3u8",
        }
    }
}

/// 重编码时的视频编码。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VideoCodec {
    #[default]
    H264,
    Hevc,
}

impl VideoCodec {
    pub const ALL: [VideoCodec; 2] = [VideoCodec::H264, VideoCodec::Hevc];

    pub fn label(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::Hevc => "H.265",
        }
    }

    /// 软件编码器名。H.265 用 libx265，体积更小但老设备可能放不动。
    pub(crate) fn software(self) -> &'static str {
        match self {
            VideoCodec::H264 => "libx264",
            VideoCodec::Hevc => "libx265",
        }
    }
}

/// 画质档位。CRF 越小越清晰、文件越大。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Quality {
    High,
    #[default]
    Standard,
    Small,
}

impl Quality {
    pub const ALL: [Quality; 3] = [Quality::High, Quality::Standard, Quality::Small];

    pub fn label(self) -> &'static str {
        match self {
            Quality::High => "高画质",
            Quality::Standard => "标准",
            Quality::Small => "小体积",
        }
    }

    pub(crate) fn crf(self) -> u8 {
        match self {
            Quality::High => 18,
            // 与之前的默认值一致：重编码一次不该悄悄改变画质。
            Quality::Standard => 20,
            Quality::Small => 23,
        }
    }
}

/// 流复制的切点对齐方向。
///
/// 流复制只能从关键帧起步，而用户输入的时间几乎总落在两个关键帧之间，
/// 于是必然要选一边吃亏：往前多含一段，还是往后少含一段。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyframeAlign {
    /// 吸附到前一个关键帧：画面比设定时间开始得早，但绝不丢用户要的内容。
    #[default]
    Previous,
    /// 吸附到后一个关键帧：画面开始得晚，但绝不包含用户不要的内容。
    Next,
}

impl KeyframeAlign {
    pub const ALL: [KeyframeAlign; 2] = [KeyframeAlign::Previous, KeyframeAlign::Next];

    pub fn label(self) -> &'static str {
        match self {
            KeyframeAlign::Previous => "向前对齐",
            KeyframeAlign::Next => "向后对齐",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            KeyframeAlign::Previous => "多包含开头一段，确保要的内容一帧不少",
            KeyframeAlign::Next => "少包含开头一段，确保不要的内容一帧不进",
        }
    }
}

/// 输出像素格式。
///
/// 位深是重编码里唯一补不回来的损失：10bit 源写成 8bit，色阶就永久断成 256 级，
/// 和 CRF、码率完全无关。所以默认跟随源，只留一个「强制 8bit」的退路。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PixelFormat {
    /// 跟随源：不写 `-pix_fmt`，由 ffmpeg 在「源格式 ∩ 编码器支持」里挑最接近的。
    /// 10bit 源得 10bit，8bit 源得 8bit。
    #[default]
    Source,
    /// 强制 yuv420p：老播放器、老电视、老手机都能解。
    Yuv420p,
}

impl PixelFormat {
    pub const ALL: [PixelFormat; 2] = [PixelFormat::Source, PixelFormat::Yuv420p];

    pub fn label(self) -> &'static str {
        match self {
            PixelFormat::Source => "跟随源",
            PixelFormat::Yuv420p => "强制 8bit",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            PixelFormat::Source => {
                "10bit / HDR 源保持 10bit，不掉色阶；硬件编码与 HLS 仍按 8bit 输出"
            }
            PixelFormat::Yuv420p => "统一 8bit 4:2:0：兼容性最好，10bit 源会掉色阶",
        }
    }
}
