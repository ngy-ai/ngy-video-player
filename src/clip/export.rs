//! 剪切片段的导出参数：剪切方式、编码、滤镜与轨道保留。
//!
//! 「精确重编码」和「无损流复制」是同一件事的两条路：前者逐帧解码再编码，
//! 切点精确但慢；后者直接搬流，快但只能落在关键帧上，起点可能比输入的时间早。
//! 两条路的参数集合完全不同，所以集中在这里拼，裁切面板只负责选。
//!
//! 滤镜与响度处理只在重编码时可用：流复制根本没有解码后的样本可以加工。

mod types;

use super::ClipRequest;
pub use types::{EncoderMode, ExportMode, OutputFormat, Quality, VideoCodec};

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 一次裁切要带上的全部导出选项。
#[derive(Clone, Debug, PartialEq)]
pub struct ExportOptions {
    pub mode: ExportMode,
    pub format: OutputFormat,
    pub codec: VideoCodec,
    /// 编码器偏好。实际选中的编码器由任务启动时探测决定。
    pub encoder_mode: EncoderMode,
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
    /// 音频偏移（秒）。正数表示声音延后、负数表示提前，用来修音画不同步。
    pub audio_offset: f64,
    /// 按音频时间戳重新采样，修长时间录制积累的漂移（音画越播越不齐）。
    pub sync_audio: bool,
    /// 强制恒定帧率。手机拍的 VFR 素材在剪辑软件里会出现音画抖动，
    /// 转成 CFR 后时间轴才稳定。
    pub force_cfr: bool,
    /// 重新生成缺失/负的时间戳。流复制也生效，适合录制中断过的 TS/FLV。
    pub fix_timestamps: bool,
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
            format: OutputFormat::Mkv,
            codec: VideoCodec::H264,
            encoder_mode: EncoderMode::Auto,
            quality: Quality::Standard,
            tonemap_hdr: false,
            deinterlace: false,
            denoise: false,
            sharpen: false,
            normalize_audio: false,
            audio_offset: 0.0,
            sync_audio: false,
            force_cfr: false,
            fix_timestamps: false,
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
    ///
    /// 音频偏移用「延迟」与「裁掉开头」两种写法实现：`adelay` 只推迟音频，
    /// 不碰画面；负数则把开头剪掉，等价于让声音提前。
    pub fn audio_filters(&self) -> Option<String> {
        if !self.filters_available() {
            return None;
        }
        let mut filters: Vec<String> = Vec::new();
        if self.sync_audio {
            // 按音频包时间戳补齐/丢弃样本，长时间录制里声卡与视频时钟的微小
            // 偏差不会越积越多。first_pts=0 让第一段从 0 开始，避免整轨前移。
            filters.push("aresample=async=1:first_pts=0".to_string());
        }
        let offset = self.audio_offset.clamp(-5.0, 5.0);
        // 偏移必须排在 loudnorm 前面：loudnorm 输出的是动态声道布局，
        // `adelay=...:all=1` 接在它后面会报「Cannot select channel layout」。
        if offset > 0.0005 {
            filters.push(format!("adelay={}:all=1", (offset * 1000.0).round() as i64));
        } else if offset < -0.0005 {
            filters.push(format!("atrim=start={:.3},asetpts=PTS-STARTPTS", -offset));
        }
        if self.normalize_audio {
            filters.push("loudnorm=I=-16:TP=-1.5:LRA=11".to_string());
        }
        (!filters.is_empty()).then(|| filters.join(","))
    }
}

/// 拼 ffmpeg 参数。
///
/// `-ss` 放在 `-i` 前：重编码时 ffmpeg 会先跳到目标附近再精确解码到目标帧，
/// 起点精确到帧，长视频也不用从第 0 秒解起；流复制时这一跳只能落在关键帧上，
/// 这也是「无损」必须接受的偏差。
pub(super) fn build_command(executable: &Path, request: &ClipRequest, encoder: &str) -> Command {
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
        .arg("pipe:1");
    if options.fix_timestamps {
        // 放在 -i 前：让解复用器为缺失的 PTS 重新生成时间戳。录制中断过的
        // TS/FLV 常见整段没有 PTS，靠输出端补救已经晚了。
        command.arg("-fflags").arg("+genpts");
    }
    command
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
    // HLS 的 TS 分片装不下字幕；就算调用方没清掉开关，这里也不再映射。
    let hls = options.format == OutputFormat::Hls;
    if options.keep_subtitles && !hls {
        command.arg("-map").arg("0:s?");
    }
    command
        .arg("-map_metadata")
        .arg(if hls { "-1" } else { "0" })
        .arg("-map_chapters")
        .arg(if !hls && options.keep_chapters {
            "0"
        } else {
            "-1"
        });

    if options.mode == ExportMode::Copy {
        // 画、音、字幕一起复制：这时候任何 `-c:v` 之类都会把「无损」两个字作废。
        command.arg("-c").arg("copy");
    } else {
        command.arg("-c:v").arg(encoder);
        for arg in super::encoder::video_args(encoder, options.quality) {
            command.arg(arg);
        }
        command.arg("-pix_fmt").arg("yuv420p");
        if let Some(filters) = options.video_filters() {
            command.arg("-vf").arg(filters);
        }
        if options.force_cfr {
            // `-fps_mode` 是 ffmpeg 5.1+ 的写法，`-vsync cfr` 在新版本会告警。
            // 不指定目标帧率时按源的名义帧率补齐/丢帧，足以治好手机 VFR 的抖动。
            command.arg("-fps_mode").arg("cfr");
        }
        if hls {
            // 关键帧落在 6 秒边界上，分片长度才和 -hls_time 一致；否则默认
            // GOP 是 250 帧，10 秒才切一片。
            command
                .arg("-force_key_frames")
                .arg("expr:gte(t,n_forced*6)");
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

    command.arg("-avoid_negative_ts").arg("make_zero");
    if hls {
        command
            .arg("-f")
            .arg("hls")
            .arg("-hls_time")
            .arg("6")
            .arg("-hls_playlist_type")
            .arg("vod")
            .arg("-hls_flags")
            .arg("independent_segments")
            .arg("-hls_segment_filename")
            .arg(hls_segment_path(&request.output));
    }
    command
        .arg(&request.output)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// HLS 分片的文件名模板。放在播放列表同目录，名字取播放列表的 stem。
fn hls_segment_path(output: &Path) -> PathBuf {
    let stem = output
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "片段".to_string());
    output.with_file_name(format!("{stem}_%04d.ts"))
}

#[cfg(test)]
mod tests;
