//! 导出参数里的枚举：剪切方式、输出格式、编码、画质、码率控制、像素格式。
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

/// 导出成单文件、切片流还是 GIF 动图。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OutputFormat {
    /// 一个 mkv 装下全部轨道。
    #[default]
    Mkv,
    /// m3u8 + 一串 .ts 分片，供网页 / 流媒体服务器直接使用。
    Hls,
    /// m3u8 + fMP4（CMAF）分片：分片本身是 mp4 盒子，切点对齐在样本上，
    /// 同一份分片能被 HLS 与 DASH 复用，是现在 CDN 上的主流写法。
    HlsFmp4,
    /// MPD + m4s 分片：网页播放器用 dash.js，与 HLS 并行的事实标准。
    Dash,
    /// 12fps 的 GIF 动图：没有声音，颜色只有 256 色，适合做表情包与网页预览。
    Gif,
}

impl OutputFormat {
    pub const ALL: [OutputFormat; 5] = [
        OutputFormat::Mkv,
        OutputFormat::Hls,
        OutputFormat::HlsFmp4,
        OutputFormat::Dash,
        OutputFormat::Gif,
    ];

    pub fn label(self) -> &'static str {
        match self {
            OutputFormat::Mkv => "单文件 mkv",
            OutputFormat::Hls => "HLS · TS 分片",
            OutputFormat::HlsFmp4 => "HLS · fMP4",
            OutputFormat::Dash => "DASH",
            OutputFormat::Gif => "GIF 动图",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            OutputFormat::Mkv => "一个文件装下画面、音轨、字幕与章节",
            OutputFormat::Hls => "输出 m3u8 与 .ts 分片；只保留画面与音轨，字幕/章节不写入",
            OutputFormat::HlsFmp4 => {
                "输出 m3u8 与 fMP4 分片，切点对齐在样本上；老设备的兼容性比 TS 略差"
            }
            OutputFormat::Dash => "输出 .mpd 与 .m4s 分片，给 dash.js 之类的网页播放器用",
            OutputFormat::Gif => {
                "12fps、480 宽的动图，无限循环；没有声音，颜色只有 256 色，适合短片段"
            }
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            OutputFormat::Mkv => "mkv",
            OutputFormat::Hls | OutputFormat::HlsFmp4 => "m3u8",
            OutputFormat::Dash => "mpd",
            OutputFormat::Gif => "gif",
        }
    }

    /// 这种格式只能重编码：GIF 没有「把原码流直接塞进去」这回事，
    /// 所以要压过用户选的「无损流复制」。
    pub fn forces_reencode(self) -> bool {
        matches!(self, OutputFormat::Gif)
    }

    /// 切片类输出：产物是一份播放列表加一堆分片。
    ///
    /// 它们都不写字幕与章节——TS 分片装不下字幕，DASH 的字幕要单独成轨；
    /// 与其写出一半，不如说清楚「只出画面与音轨」。
    pub fn is_segmented(self) -> bool {
        matches!(
            self,
            OutputFormat::Hls | OutputFormat::HlsFmp4 | OutputFormat::Dash
        )
    }

    pub fn is_hls(self) -> bool {
        matches!(self, OutputFormat::Hls | OutputFormat::HlsFmp4)
    }
}

/// 输出色彩空间。
///
/// 「跟随源」不动；显式指定时用 zscale 做真正的矩阵/原色/传递函数转换，
/// 而不是只改标签——只改标签会让播放器按错的色彩解释画面，偏色更严重。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorMatrix {
    #[default]
    Source,
    Bt709,
    Bt2020,
}

impl ColorMatrix {
    pub const ALL: [ColorMatrix; 3] =
        [ColorMatrix::Source, ColorMatrix::Bt709, ColorMatrix::Bt2020];

    pub fn label(self) -> &'static str {
        match self {
            ColorMatrix::Source => "跟随源",
            ColorMatrix::Bt709 => "BT.709",
            ColorMatrix::Bt2020 => "BT.2020",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            ColorMatrix::Source => "不改色彩，最省事也最安全",
            ColorMatrix::Bt709 => "SDR 与网页/手机的通行标准；把 2020 素材转回来才不偏色",
            ColorMatrix::Bt2020 => "广色域；目标设备不支持时会显得灰淡",
        }
    }

    /// zscale 的参数片段。
    pub(super) fn zscale(self) -> Option<&'static str> {
        match self {
            ColorMatrix::Source => None,
            ColorMatrix::Bt709 => Some("m=bt709:p=bt709:t=bt709"),
            ColorMatrix::Bt2020 => Some("m=bt2020:p=bt2020:t=bt2020"),
        }
    }
}

/// 输出色彩范围。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorRange {
    #[default]
    Source,
    /// 电视范围 16–235：广播与大多数视频文件的标准。
    Tv,
    /// 全范围 0–255：部分录屏、手机与游戏素材。
    Full,
}

impl ColorRange {
    pub const ALL: [ColorRange; 3] = [ColorRange::Source, ColorRange::Tv, ColorRange::Full];

    pub fn label(self) -> &'static str {
        match self {
            ColorRange::Source => "跟随源",
            ColorRange::Tv => "电视 16–235",
            ColorRange::Full => "全范围 0–255",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            ColorRange::Source => "不动范围",
            ColorRange::Tv => "黑场发灰/高光死白时换一档试试，多半是源被标错了",
            ColorRange::Full => "录屏与游戏素材多用这个范围",
        }
    }

    pub(super) fn zscale(self) -> Option<&'static str> {
        match self {
            ColorRange::Source => None,
            ColorRange::Tv => Some("range=limited"),
            ColorRange::Full => Some("range=full"),
        }
    }
}

/// 码率控制方式。
///
/// 「质量档」是默认：软件编码走 CRF，硬件编码按档位给目标码率，体积与画质都随
/// 内容变化。需要**体积可预测**时才切到「固定码率」——比如上传平台有码率上限，
/// 或者要往一张盘里塞满若干小时。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RateControl {
    #[default]
    Quality,
    Bitrate,
}

impl RateControl {
    pub const ALL: [RateControl; 2] = [RateControl::Quality, RateControl::Bitrate];

    pub fn label(self) -> &'static str {
        match self {
            RateControl::Quality => "质量优先",
            RateControl::Bitrate => "固定码率",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            RateControl::Quality => "按画面复杂度自动分配码率，同样画质下体积最小",
            RateControl::Bitrate => "码率钉死，体积可预估；剧烈运动场景画质会掉",
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

/// 光流插帧的目标帧率。
///
/// 只留电影与直播常见的几档：`minterpolate` 是整条链里最慢的一步，
/// 60 → 120 的耗时还会再翻倍，档位越多越容易让人误选。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FrameRate {
    F24,
    F30,
    #[default]
    F60,
    F120,
}

impl FrameRate {
    pub const ALL: [FrameRate; 4] = [
        FrameRate::F24,
        FrameRate::F30,
        FrameRate::F60,
        FrameRate::F120,
    ];

    pub fn fps(self) -> u32 {
        match self {
            FrameRate::F24 => 24,
            FrameRate::F30 => 30,
            FrameRate::F60 => 60,
            FrameRate::F120 => 120,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FrameRate::F24 => "24",
            FrameRate::F30 => "30",
            FrameRate::F60 => "60",
            FrameRate::F120 => "120",
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
