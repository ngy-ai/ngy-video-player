//! 剪切片段的导出参数：剪切方式、编码、滤镜与轨道保留。
//!
//! 「精确重编码」和「无损流复制」是同一件事的两条路：前者逐帧解码再编码，
//! 切点精确但慢；后者直接搬流，快但只能落在关键帧上，起点可能比输入的时间早。
//! 两条路的参数集合完全不同，所以集中在这里拼，裁切面板只负责选。
//!
//! 滤镜与响度处理只在重编码时可用：流复制根本没有解码后的样本可以加工。

use super::ClipRequest;
use std::path::Path;
use std::process::{Command, Stdio};

/// 剪切方式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExportMode {
    /// 逐帧重编码：切点精确到帧，兼容性最好。
    #[default]
    Exact,
    /// 流复制：不动画面与音频，速度快，切点对齐到前一个关键帧。
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
            ExportMode::Copy => "不重新编码，几秒完成；起点会对齐到前一个关键帧，滤镜与响度不可用",
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

    /// ffmpeg 编码器名。H.265 用 libx265，体积更小但老设备可能放不动。
    fn encoder(self) -> &'static str {
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

    fn crf(self) -> u8 {
        match self {
            Quality::High => 18,
            // 与之前的默认值一致：重编码一次不该悄悄改变画质。
            Quality::Standard => 20,
            Quality::Small => 23,
        }
    }
}

/// 一次裁切要带上的全部导出选项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportOptions {
    pub mode: ExportMode,
    pub codec: VideoCodec,
    pub quality: Quality,
    /// HDR（PQ/HLG）转 SDR：把高光压进 BT.709，否则导出的画面会发灰。
    pub tonemap_hdr: bool,
    /// 去隔行。老电视录制、DVD 转录常见。
    pub deinterlace: bool,
    /// 轻量降噪，代价是细节变软。
    pub denoise: bool,
    /// 锐化。适合重编码后偏软的片源。
    pub sharpen: bool,
    /// EBU R128 响度标准化，导出后音量与常见平台一致。
    pub normalize_audio: bool,
    /// 音轨：一条不留时只导出画面与字幕。
    pub keep_audio: bool,
    /// 字幕：点阵字幕会原样复制，文本字幕按容器支持转码。
    pub keep_subtitles: bool,
    /// 章节与容器元数据。
    pub keep_chapters: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            mode: ExportMode::Exact,
            codec: VideoCodec::H264,
            quality: Quality::Standard,
            tonemap_hdr: false,
            deinterlace: false,
            denoise: false,
            sharpen: false,
            normalize_audio: false,
            keep_audio: true,
            keep_subtitles: true,
            keep_chapters: true,
        }
    }
}

impl ExportOptions {
    /// 流复制时滤镜与响度没有意义，界面据此把那些开关置灰。
    pub fn filters_available(&self) -> bool {
        self.mode == ExportMode::Exact
    }

    /// 视频滤镜链。顺序按「先修时间/色彩、再修噪点、最后锐化」排。
    pub fn video_filters(&self) -> Option<String> {
        if !self.filters_available() {
            return None;
        }
        let mut filters: Vec<&str> = Vec::new();
        if self.deinterlace {
            filters.push("yadif=mode=send_frame:parity=auto:deint=all");
        }
        if self.tonemap_hdr {
            // zscale 在线性光下做色调映射，最后回到 BT.709 的 limited range。
            // npl=100 是常见 SDR 参考白；hable 曲线在保留高光细节上比 clip 稳。
            filters.extend([
                "zscale=t=linear:npl=100",
                "format=gbrpf32le",
                "zscale=p=bt709",
                "tonemap=tonemap=hable:desat=0",
                "zscale=t=bt709:m=bt709:r=tv",
                "format=yuv420p",
            ]);
        }
        if self.denoise {
            filters.push("hqdn3d=4:3:6:4.5");
        }
        if self.sharpen {
            filters.push("unsharp=5:5:0.8:3:3:0.4");
        }
        (!filters.is_empty()).then(|| filters.join(","))
    }

    /// 音频滤镜。响度标准化按单遍处理：裁切出来的片段通常不长，两遍扫描
    /// 要多解一遍音频，收益不明显。
    pub fn audio_filters(&self) -> Option<&'static str> {
        (self.filters_available() && self.normalize_audio)
            .then_some("loudnorm=I=-16:TP=-1.5:LRA=11")
    }
}

/// 拼 ffmpeg 参数。
///
/// `-ss` 放在 `-i` 前：重编码时 ffmpeg 会先跳到目标附近再精确解码到目标帧，
/// 起点精确到帧，长视频也不用从第 0 秒解起；流复制时这一跳只能落在关键帧上，
/// 这也是「无损」必须接受的偏差。
pub(super) fn build_command(executable: &Path, request: &ClipRequest) -> Command {
    let options = &request.options;
    let mut command = Command::new(executable);
    command
        .arg("-hide_banner")
        .arg("-nostdin")
        .arg("-y")
        .arg("-nostats")
        .arg("-loglevel")
        .arg("error")
        .arg("-progress")
        .arg("pipe:1")
        .arg("-ss")
        .arg(format!("{:.3}", request.range.start))
        .arg("-i")
        .arg(&request.input)
        .arg("-t")
        .arg(format!("{:.3}", request.range.duration()))
        // 大写 V 排除封面图（attached_pic），否则第一路「视频」可能是一张 jpeg。
        .arg("-map")
        .arg("0:V:0?");
    if options.keep_audio {
        command.arg("-map").arg("0:a?");
    }
    if options.keep_subtitles {
        command.arg("-map").arg("0:s?");
    }
    command
        .arg("-map_metadata")
        .arg("0")
        .arg("-map_chapters")
        .arg(if options.keep_chapters { "0" } else { "-1" });

    if options.mode == ExportMode::Copy {
        // 画、音、字幕一起复制：这时候任何 `-c:v` 之类都会把「无损」两个字作废。
        command.arg("-c").arg("copy");
    } else {
        command
            .arg("-c:v")
            .arg(options.codec.encoder())
            .arg("-preset")
            .arg("medium")
            .arg("-crf")
            .arg(options.quality.crf().to_string())
            .arg("-pix_fmt")
            .arg("yuv420p");
        if let Some(filters) = options.video_filters() {
            command.arg("-vf").arg(filters);
        }
        if options.keep_audio {
            command.arg("-c:a").arg("aac").arg("-b:a").arg("192k");
            if let Some(filters) = options.audio_filters() {
                // loudnorm 内部工作在 192kHz，不指定采样率时输出会被它顶上去。
                command.arg("-af").arg(filters).arg("-ar").arg("48000");
            }
        }
        if options.keep_subtitles {
            command.arg("-c:s").arg("copy");
        }
    }

    command
        .arg("-avoid_negative_ts")
        .arg("make_zero")
        .arg(&request.output)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::ClipRange;
    use std::path::PathBuf;

    fn request(options: ExportOptions) -> ClipRequest {
        ClipRequest {
            input: PathBuf::from("movie.mkv"),
            output: PathBuf::from("movie - 片段.mkv"),
            range: ClipRange {
                start: 10.0,
                end: 25.0,
            },
            options,
        }
    }

    fn args(options: ExportOptions) -> Vec<String> {
        build_command(Path::new("ffmpeg"), &request(options))
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn has(args: &[String], needle: &[&str]) -> bool {
        args.windows(needle.len()).any(|slice| {
            slice
                .iter()
                .zip(needle)
                .all(|(arg, want)| arg.as_str() == *want)
        })
    }

    fn count(args: &[String], value: &str) -> usize {
        args.iter().filter(|arg| arg.as_str() == value).count()
    }

    #[test]
    fn 默认仍然精确重编码为_h264() {
        let args = args(ExportOptions::default());
        assert!(has(&args, &["-c:v", "libx264"]));
        assert!(has(&args, &["-crf", "20"]));
        assert!(has(&args, &["-c:a", "aac"]));
        assert!(has(&args, &["-c:s", "copy"]));
        assert!(!args.iter().any(|arg| arg == "-vf"));
        assert!(!args.iter().any(|arg| arg == "-af"));
        assert!(has(&args, &["-map", "0:a?"]));
        assert!(has(&args, &["-map", "0:s?"]));
        assert!(has(&args, &["-map_chapters", "0"]));
    }

    #[test]
    fn 无损流复制不带任何编码与滤镜参数() {
        let mut options = ExportOptions {
            mode: ExportMode::Copy,
            ..ExportOptions::default()
        };
        // 即使界面状态里滤镜还开着，流复制也必须忽略它们。
        options.tonemap_hdr = true;
        options.deinterlace = true;
        options.normalize_audio = true;
        let args = args(options);
        assert!(has(&args, &["-c", "copy"]));
        for forbidden in ["-c:v", "-crf", "-vf", "-af", "-c:a", "-c:s"] {
            assert!(
                !args.iter().any(|arg| arg == forbidden),
                "流复制不该带 {forbidden}"
            );
        }
        assert!(has(&args, &["-map", "0:a?"]));
    }

    #[test]
    fn 画质档位换算成_crf() {
        let crf = |quality| {
            let options = ExportOptions {
                quality,
                ..ExportOptions::default()
            };
            args(options)
                .windows(2)
                .find(|pair| pair[0] == "-crf")
                .map(|pair| pair[1].clone())
                .unwrap()
        };
        assert_eq!(crf(Quality::High), "18");
        assert_eq!(crf(Quality::Standard), "20");
        assert_eq!(crf(Quality::Small), "23");
    }

    #[test]
    fn 滤镜按固定顺序串成一条链() {
        let options = ExportOptions {
            deinterlace: true,
            tonemap_hdr: true,
            denoise: true,
            sharpen: true,
            ..ExportOptions::default()
        };
        let filters = options.video_filters().unwrap();
        let deinterlace = filters.find("yadif").unwrap();
        let tonemap = filters.find("tonemap=tonemap=hable").unwrap();
        let denoise = filters.find("hqdn3d").unwrap();
        let sharpen = filters.find("unsharp").unwrap();
        assert!(deinterlace < tonemap && tonemap < denoise && denoise < sharpen);
        // 色调映射必须落回 yuv420p，否则后面的降噪/锐化会拿到浮点线性帧。
        assert!(filters.contains("tonemap=tonemap=hable:desat=0"));
        assert!(filters.contains("zscale=t=bt709:m=bt709:r=tv,format=yuv420p"));
    }

    #[test]
    fn 响度标准化会补上采样率() {
        let options = ExportOptions {
            normalize_audio: true,
            ..ExportOptions::default()
        };
        let args = args(options);
        assert!(has(&args, &["-af", "loudnorm=I=-16:TP=-1.5:LRA=11"]));
        assert!(has(&args, &["-ar", "48000"]));
    }

    #[test]
    fn 去掉音轨后不再有音频映射与编码() {
        let options = ExportOptions {
            keep_audio: false,
            ..ExportOptions::default()
        };
        let args = args(options);
        assert!(!has(&args, &["-map", "0:a?"]));
        assert!(!args.iter().any(|arg| arg == "-c:a"));
        assert!(!args.iter().any(|arg| arg == "-af"));
        assert_eq!(count(&args, "-map"), 2, "只留视频与字幕两路映射");
    }

    #[test]
    fn 去掉字幕与章节时对应参数一起消失() {
        let options = ExportOptions {
            keep_subtitles: false,
            keep_chapters: false,
            ..ExportOptions::default()
        };
        let args = args(options);
        assert!(!has(&args, &["-map", "0:s?"]));
        assert!(!args.iter().any(|arg| arg == "-c:s"));
        assert!(has(&args, &["-map_chapters", "-1"]));
    }

    #[test]
    fn 选_h265_时换编码器() {
        let options = ExportOptions {
            codec: VideoCodec::Hevc,
            ..ExportOptions::default()
        };
        let args = args(options);
        assert!(has(&args, &["-c:v", "libx265"]));
    }

    #[test]
    fn 剪切区间落在输入之后与输出之前() {
        let args = args(ExportOptions::default());
        let input = args.iter().position(|arg| arg == "-i").unwrap();
        let start = args.iter().position(|arg| arg == "-ss").unwrap();
        let end = args.iter().position(|arg| arg == "-t").unwrap();
        assert!(start < input, "-ss 必须在 -i 前才能快点开始");
        assert!(input < end, "-t 是输出时长，跟在输入之后");
        assert_eq!(args[input + 1], "movie.mkv");
        assert_eq!(args[start + 1], "10.000");
        assert_eq!(args[end + 1], "15.000");
    }
}
