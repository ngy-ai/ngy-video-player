//! 导出参数：编码器、画质、速度与音频码率。
//!
//! 与 `plan.rs` 分工：那边只管把序列展开成滤镜图，这里只管「最后用哪套参数编码」。
//! 这个结构同时也是导出预设存进 `render-presets.json` 的形状，所以每个字段都必须能
//! 安全往返：文件里出现这一版不认识的取值（新版本写的编码器、手改坏的名字）时，
//! 降级成默认值，而不是让整份预设读不出来。
//!
//! 第一版只给软件编码器：硬件编码的参数名不通用（NVENC 用 `-cq`、QSV 用 `-global_quality`），
//! 混进同一个结构会让「预设」变成一张猜谜表。要支持硬件编码时另开一档 `encoder`，
//! 由 `encoder::pick` 探测可用性。

use serde::{Deserialize, Serialize};

/// 视频编码器。存盘写字符串（`"h264"`），认不出来的落回 `H264`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", from = "String")]
pub enum VideoCodec {
    /// 兼容性最好：浏览器、手机、旧播放器都认。
    #[default]
    H264,
    /// 同画质下体积约小一半，解码更吃 CPU；装进 mp4 时要打 `hvc1` 标。
    H265,
}

impl VideoCodec {
    pub fn id(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::H265 => "h265",
        }
    }

    /// 认不出来的名字回退 H.264：编码器不存在时 ffmpeg 会直接失败，
    /// 而降级成最通用的那个至少能出片。
    fn from_id(id: &str) -> Self {
        match id {
            "h265" | "hevc" => Self::H265,
            _ => Self::H264,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::H265 => "H.265",
        }
    }

    /// ffmpeg 的编码器名。
    fn encoder(self) -> &'static str {
        match self {
            Self::H264 => "libx264",
            Self::H265 => "libx265",
        }
    }

    /// mp4 里的编码标签。H.265 不打 `hvc1` 时 QuickTime / 部分手机认不出这条轨。
    fn tag(self) -> Option<&'static str> {
        match self {
            Self::H264 => None,
            Self::H265 => Some("hvc1"),
        }
    }
}

impl From<VideoCodec> for String {
    fn from(codec: VideoCodec) -> Self {
        codec.id().to_string()
    }
}

impl From<String> for VideoCodec {
    fn from(id: String) -> Self {
        Self::from_id(&id)
    }
}

/// 画质档位：映射到 x264 / x265 的 CRF。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", from = "String")]
pub enum Quality {
    /// 大致「看不出压过」，母版或要再剪一次的素材用这一档。
    High,
    /// 默认：发布到网上的成片。
    #[default]
    Standard,
    /// 体积优先：预览片、发给别人看的草稿。
    Small,
}

impl Quality {
    pub fn id(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Standard => "standard",
            Self::Small => "small",
        }
    }

    fn from_id(id: &str) -> Self {
        match id {
            "high" => Self::High,
            "small" => Self::Small,
            _ => Self::Standard,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::High => "高画质",
            Self::Standard => "标准",
            Self::Small => "小体积",
        }
    }

    /// CRF 越小越清晰、体积越大。标准档保持 18——这是这一版之前写死的值，
    /// 老工程与既有导出测试因此不会有任何变化。
    fn crf(self) -> u32 {
        match self {
            Self::High => 16,
            Self::Standard => 18,
            Self::Small => 23,
        }
    }
}

impl From<Quality> for String {
    fn from(quality: Quality) -> Self {
        quality.id().to_string()
    }
}

impl From<String> for Quality {
    fn from(id: String) -> Self {
        Self::from_id(&id)
    }
}

/// 编码速度：x264 / x265 的 `-preset`。越慢压得越好，但编码时间成倍增长。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", from = "String")]
pub enum EncodeSpeed {
    /// 赶时间：同样的 CRF 会大一点。
    Fast,
    /// 默认：速度与体积的平衡点。
    #[default]
    Medium,
    /// 交付前最后一版：慢，但同画质最省空间。
    Slow,
}

impl EncodeSpeed {
    pub fn id(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Medium => "medium",
            Self::Slow => "slow",
        }
    }

    fn from_id(id: &str) -> Self {
        match id {
            "fast" => Self::Fast,
            "slow" => Self::Slow,
            _ => Self::Medium,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Fast => "快",
            Self::Medium => "标准",
            Self::Slow => "慢",
        }
    }

    fn preset(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Medium => "medium",
            Self::Slow => "slow",
        }
    }
}

impl From<EncodeSpeed> for String {
    fn from(speed: EncodeSpeed) -> Self {
        speed.id().to_string()
    }
}

impl From<String> for EncodeSpeed {
    fn from(id: String) -> Self {
        Self::from_id(&id)
    }
}

/// 一整套导出参数。
///
/// `#[serde(default)]` 是向前兼容的一半：老文件缺字段（比如以后才加的 `faststart`）
/// 读出来是默认值，而不是整条预设作废；另一半在枚举的 `from_id` 里。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportSettings {
    pub codec: VideoCodec,
    pub quality: Quality,
    pub speed: EncodeSpeed,
    /// 音频码率（kbps）。只在有音轨时才写进命令。
    pub audio_bitrate: u32,
    /// mp4 的 `+faststart`：把索引挪到文件头，网页播放不用等整段下载。
    pub faststart: bool,
}

impl Default for ExportSettings {
    /// 默认值就是这一版之前写死在 `plan.rs` 里的那一套（libx264 / crf18 / medium / 192k），
    /// 老工程与既有导出测试因此不会有任何变化。
    fn default() -> Self {
        Self {
            codec: VideoCodec::H264,
            quality: Quality::Standard,
            speed: EncodeSpeed::Medium,
            audio_bitrate: 192,
            faststart: true,
        }
    }
}

impl ExportSettings {
    /// 音频码率规整到 64–512 kbps：手改的文件里可能是 0 或者离谱的大数。
    fn audio_kbps(&self) -> u32 {
        self.audio_bitrate.clamp(64, 512)
    }

    /// 输出参数（不含输出文件本身）。`has_audio` 决定写音频编码还是 `-an`。
    pub fn output_args(&self, has_audio: bool) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "-c:v".into(),
            self.codec.encoder().into(),
            "-preset".into(),
            self.speed.preset().into(),
            "-crf".into(),
            self.quality.crf().to_string(),
            "-pix_fmt".into(),
            "yuv420p".into(),
        ];
        if let Some(tag) = self.codec.tag() {
            args.extend(["-tag:v".into(), tag.into()]);
        }
        if has_audio {
            args.extend([
                "-c:a".into(),
                "aac".into(),
                "-b:a".into(),
                format!("{}k", self.audio_kbps()),
            ]);
        } else {
            args.push("-an".into());
        }
        if self.faststart {
            args.extend(["-movflags".into(), "+faststart".into()]);
        }
        args
    }

    /// 面板与提示里的一行摘要。
    pub fn summary(&self) -> String {
        format!(
            "{} · {} · 编码{} · 音频 {}kbps",
            self.codec.label(),
            self.quality.label(),
            self.speed.label(),
            self.audio_kbps()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 默认参数与之前的写死值一致() {
        let args = ExportSettings::default().output_args(true);
        assert_eq!(
            args,
            [
                "-c:v",
                "libx264",
                "-preset",
                "medium",
                "-crf",
                "18",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-b:a",
                "192k",
                "-movflags",
                "+faststart",
            ]
            .map(str::to_string)
        );
    }

    #[test]
    fn 三档画质映射到不同_crf() {
        let crf = |quality: Quality| {
            let settings = ExportSettings {
                quality,
                ..ExportSettings::default()
            };
            let args = settings.output_args(false);
            let index = args.iter().position(|arg| arg == "-crf").expect("要有 crf");
            args[index + 1].clone()
        };
        assert_eq!(crf(Quality::High), "16");
        assert_eq!(crf(Quality::Standard), "18");
        assert_eq!(crf(Quality::Small), "23");
        assert_ne!(crf(Quality::High), crf(Quality::Standard));
        assert_ne!(crf(Quality::Standard), crf(Quality::Small));
    }

    #[test]
    fn 没有音轨时写_an() {
        let args = ExportSettings::default().output_args(false);
        assert!(args.contains(&"-an".to_string()));
        assert!(!args.contains(&"-c:a".to_string()));
    }

    #[test]
    fn h265_带_hvc1_标() {
        let settings = ExportSettings {
            codec: VideoCodec::H265,
            ..ExportSettings::default()
        };
        let args = settings.output_args(true);
        assert!(args.contains(&"libx265".to_string()));
        let tag = args
            .iter()
            .position(|arg| arg == "-tag:v")
            .expect("要有标签");
        assert_eq!(args[tag + 1], "hvc1");
    }

    #[test]
    fn 关掉_faststart_就不写() {
        let settings = ExportSettings {
            faststart: false,
            ..ExportSettings::default()
        };
        assert!(
            !settings
                .output_args(true)
                .contains(&"-movflags".to_string())
        );
    }

    #[test]
    fn 音频码率夹到合理区间() {
        let low = ExportSettings {
            audio_bitrate: 0,
            ..ExportSettings::default()
        };
        assert!(low.output_args(true).contains(&"64k".to_string()));
        let high = ExportSettings {
            audio_bitrate: 9999,
            ..ExportSettings::default()
        };
        assert!(high.output_args(true).contains(&"512k".to_string()));
    }

    #[test]
    fn 未知编码器降级成_h264() {
        // 新版本写进来的编码器名，老版本读不懂时不能整条预设作废。
        let settings: ExportSettings =
            serde_json::from_str(r#"{"codec":"av1","quality":"high"}"#).expect("能读出");
        assert_eq!(settings.codec, VideoCodec::H264);
        assert_eq!(settings.quality, Quality::High);
    }

    #[test]
    fn 缺字段时用默认值() {
        let settings: ExportSettings = serde_json::from_str("{}").expect("能读出");
        assert_eq!(settings, ExportSettings::default());
    }

    #[test]
    fn 往返_json_不丢字段() {
        let settings = ExportSettings {
            codec: VideoCodec::H265,
            quality: Quality::Small,
            speed: EncodeSpeed::Slow,
            audio_bitrate: 256,
            faststart: false,
        };
        let text = serde_json::to_string(&settings).unwrap();
        assert!(
            text.contains("\"h265\"") && text.contains("\"small\"") && text.contains("\"slow\""),
            "{text}"
        );
        let parsed: ExportSettings = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, settings);
    }

    #[test]
    fn 摘要把四段都说出来() {
        let summary = ExportSettings::default().summary();
        assert!(summary.contains("H.264"));
        assert!(summary.contains("标准"));
        assert!(summary.contains("192kbps"));
    }
}
