//! 剪切片段的导出参数：剪切方式、编码、滤镜与轨道保留。
//!
//! 「精确重编码」和「无损流复制」是同一件事的两条路：前者逐帧解码再编码，
//! 切点精确但慢；后者直接搬码流，快但只能从关键帧起步，实际起点由
//! `clip::keyframe` 探测后替换。两条路的参数集合完全不同，所以集中在这里拼，
//! 裁切面板只负责选。
//!
//! 滤镜与响度处理只在重编码时可用：流复制根本没有解码后的样本可以加工。

mod effects;
mod transform;
mod types;

use super::ClipRequest;
use super::loudness::Loudness;
// 编码器偏好是裁切与格式转换共用的，定义在 `crate::encoder`，这里只重新导出，
// 保持 `clip::EncoderMode` 这个既有入口不变。
pub use crate::encoder::EncoderMode;
pub use effects::{Watermark, WatermarkPosition};
pub use transform::{Crop, Edge, Pad, Rotate, Scale, VideoTransform};
pub use types::{
    ColorMatrix, ColorRange, ExportMode, FrameRate, KeyframeAlign, OutputFormat, PixelFormat,
    Quality, RateControl, VideoCodec,
};

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 一次裁切要带上的全部导出选项。
#[derive(Clone, Debug, PartialEq)]
pub struct ExportOptions {
    pub mode: ExportMode,
    /// 流复制时起点吸附到哪一侧的关键帧。`Exact` 模式下不生效。
    pub keyframe_align: KeyframeAlign,
    pub format: OutputFormat,
    pub codec: VideoCodec,
    /// 编码器偏好。实际选中的编码器由任务启动时探测决定。
    pub encoder_mode: EncoderMode,
    pub quality: Quality,
    /// 码率控制方式。见 [`RateControl`]。
    pub rate_control: RateControl,
    /// 自定义 CRF。`None` 表示按画质档换算；软件编码器才认这个值。
    pub crf: Option<u8>,
    /// 固定码率（kbps），只在 [`RateControl::Bitrate`] 下生效。
    pub bitrate_kbps: u32,
    /// 输出像素格式。默认跟随源，10bit 不会被悄悄压成 8bit。
    pub pixel_format: PixelFormat,
    /// HDR（PQ/HLG）转 SDR：把高光压进 BT.709，否则导出的画面会发灰。
    pub tonemap_hdr: bool,
    /// 输出色彩空间。见 [`ColorMatrix`]。
    pub color_matrix: ColorMatrix,
    /// 输出色彩范围。见 [`ColorRange`]。
    pub color_range: ColorRange,
    /// 按 BT.709 重写输出文件的色彩标签。
    ///
    /// 只改标签不改像素，专门对付「标签写错导致偏色」的源；和上面两项的
    /// 「真转换」是两件事，可以一起用。
    pub tag_rec709: bool,
    /// 去隔行。老电视录制、DVD 转录常见。
    pub deinterlace: bool,
    /// 轻量降噪，代价是细节变软。
    pub denoise: bool,
    /// 锐化。适合重编码后偏软的片源。
    pub sharpen: bool,
    /// 手持防抖。用 `deshake` 的单遍版本：不需要像 `vidstab` 那样先跑一遍
    /// 分析再出结果，代价是对大幅抖动只能压住一部分。
    pub stabilize: bool,
    /// 光流插帧：靠运动估计补出中间帧，把源帧率抬到 [`FrameRate`]。
    ///
    /// 只适合画面平滑的素材（动画、慢镜头）；快速剪辑会出现明显的涂抹，
    /// 而且这一步是整条滤镜链里最慢的。
    pub interpolate: bool,
    /// 插帧的目标帧率。
    pub interpolate_fps: FrameRate,
    /// 画面变换：裁切、缩放、补边、旋转、翻转。只在重编码时生效。
    pub transform: VideoTransform,
    /// 音频不重编码，直接搬原码流。
    ///
    /// 用于保住 DTS-HD / TrueHD 这类高码率音轨：转成 AAC 192k 是听得出的降级。
    /// 代价是响度、偏移、漂移校正全部失效——没有解码后的样本可以加工。
    pub audio_copy: bool,
    /// EBU R128 响度标准化，导出后音量与常见平台一致。
    pub normalize_audio: bool,
    /// 两遍扫描：先量一遍整段响度，再按测量值做线性增益。
    ///
    /// 单遍 loudnorm 是「边统计边调」，片源忽大忽小时会先压后放；两遍不压动态，
    /// 代价是多解码一遍音频，长片会明显变慢。
    pub two_pass_loudness: bool,
    /// 音量增益（dB）。用来把整体抬高/压低几 dB，和响度标准化是两件事：
    /// 前者不改变动态，后者会把响度拉到目标值，两个都开时增益作用在标准化之后。
    pub volume_db: f32,
    /// 输出采样率（Hz）。`None` 跟随源；挂了滤镜时 ffmpeg 默认会给 48kHz，
    /// 这里允许显式指定，老播放器（16kHz 语音、车载）才吃得下。
    pub sample_rate: Option<u32>,
    /// 输出声道数。`None` 跟随源，1 单声道，2 立体声。
    /// 单声道能把对白类内容的体积压掉近一半，也不影响收听。
    pub channels: Option<u8>,
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
    /// 倒放：整段反过来播。
    ///
    /// 画面用 `reverse`、声音用 `areverse`，两者都必须重编码；`reverse` 会把整段
    /// 解码后的帧攒在内存里，长片会直接把内存吃光，面板上只留给短片用。
    pub reverse: bool,
    /// 开头定格（秒）：把首帧冻住这么久再开始播，用来做标题卡。
    /// `0` 表示不冻结。音频同步推迟同样的时长。
    pub freeze_lead: f32,
    /// 文字水印。
    pub watermark: Watermark,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            mode: ExportMode::Exact,
            keyframe_align: KeyframeAlign::Previous,
            format: OutputFormat::Mkv,
            codec: VideoCodec::H264,
            encoder_mode: EncoderMode::Auto,
            quality: Quality::Standard,
            rate_control: RateControl::Quality,
            crf: None,
            bitrate_kbps: DEFAULT_BITRATE_KBPS,
            pixel_format: PixelFormat::Source,
            tonemap_hdr: false,
            color_matrix: ColorMatrix::Source,
            color_range: ColorRange::Source,
            tag_rec709: false,
            deinterlace: false,
            denoise: false,
            sharpen: false,
            stabilize: false,
            interpolate: false,
            interpolate_fps: FrameRate::default(),
            transform: VideoTransform::default(),
            audio_copy: false,
            normalize_audio: false,
            two_pass_loudness: false,
            volume_db: 0.0,
            sample_rate: None,
            channels: None,
            audio_offset: 0.0,
            sync_audio: false,
            force_cfr: false,
            fix_timestamps: false,
            keep_audio: true,
            keep_subtitles: true,
            keep_chapters: true,
            reverse: false,
            freeze_lead: 0.0,
            watermark: Watermark::default(),
        }
    }
}

impl ExportOptions {
    /// 流复制时滤镜与响度没有意义，界面据此把那些开关置灰。
    ///
    /// GIF 是例外：它只能重编码，用户选了什么方式都得按重编码走。
    pub fn filters_available(&self) -> bool {
        self.mode == ExportMode::Exact || self.format.forces_reencode()
    }

    /// 这次导出是否真的把音频原样搬过去。
    ///
    /// HLS 的 TS 分片只认 AAC / AC3 / MP3，源是 FLAC、Opus 时复制会让整批导出失败，
    /// 所以这条路上勾了也按转码处理。
    fn copies_audio(&self) -> bool {
        // 倒放要 `areverse`，样本必须解出来；勾了「音频不重编码」也得作废。
        self.audio_copy && self.format == OutputFormat::Mkv && !self.reverse
    }

    /// 音频还能不能挂滤镜。`-c:a copy` 时样本根本不解出来，`-af` 会被忽略，
    /// 界面据此把响度、偏移、漂移校正收起来，而不是让用户勾了没反应。
    pub fn audio_filters_available(&self) -> bool {
        self.filters_available() && !self.copies_audio()
    }

    /// 视频滤镜链。顺序按「先修时间/色彩、再动画面几何、再修噪点、最后锐化」排。
    ///
    /// 几何变换放在去噪之前：先裁掉黑边，降噪就不用在内含黑边的整帧上白跑一遍，
    /// 缩到小尺寸后再降噪也快得多。
    pub fn video_filters(&self) -> Option<String> {
        if !self.filters_available() {
            return None;
        }
        let mut filters: Vec<String> = Vec::new();
        if self.deinterlace {
            filters.push("yadif=mode=send_frame:parity=auto:deint=all".to_string());
        }
        if self.tonemap_hdr {
            // zscale 在线性光下做色调映射，最后回到 BT.709 的 limited range。
            // npl=100 是常见 SDR 参考白；hable 曲线在保留高光细节上比 clip 稳。
            filters.extend(
                [
                    "zscale=t=linear:npl=100",
                    "format=gbrpf32le",
                    "zscale=p=bt709",
                    "tonemap=tonemap=hable:desat=0",
                    "zscale=t=bt709:m=bt709:r=tv",
                    "format=yuv420p",
                ]
                .map(String::from),
            );
        }
        // 色彩转换排在几何变换之前：先统一到目标色彩空间，缩放与裁切才不会在
        // 半转换的像素上做插值。
        if let Some(color) = self.color_filters() {
            filters.push(color);
        }
        filters.extend(self.transform.filters());
        // 倒放排在几何变换之后：`reverse` 会把整段帧攒在内存里，先缩到目标尺寸
        // 再攒，占用能少一个数量级。
        if self.reverse {
            filters.push("reverse".to_string());
        }
        if self.freeze_seconds() > 0.0 {
            filters.push(format!(
                "tpad=start_mode=clone:start_duration={:.3}",
                self.freeze_seconds()
            ));
        }
        // 防抖排在降噪之前：先把抖动的位移抹平，再在稳定的画面上降噪，
        // 否则每一帧的噪点位置都在动，降噪会把它们当成细节保留下来。
        if self.stabilize {
            // `edge=mirror` 让移出画面的边缘用镜像补齐，默认的黑色边框
            // 在抖动幅度大时会露出黑边。
            filters.push("deshake=edge=mirror".to_string());
        }
        if self.denoise {
            filters.push("hqdn3d=4:3:6:4.5".to_string());
        }
        if self.sharpen {
            filters.push("unsharp=5:5:0.8:3:3:0.4".to_string());
        }
        // 插帧要在所有逐帧处理之后：先修好每一帧，再去生成中间帧；
        // 反过来的话插出来的帧又会成为下一步的输入，代价翻倍。
        if self.interpolate {
            filters.push(format!(
                "minterpolate=fps={}:mi_mode=mci:mc_mode=aobmc:vsbmc=1",
                self.interpolate_fps.fps()
            ));
        }
        // 水印压在最后：几何变换之后再画，坐标才是最终画面里的坐标；
        // 排在插帧后面，文字也不会被运动估计抹出残影。
        if let Some(watermark) = self.watermark.filter() {
            filters.push(watermark);
        }
        (!filters.is_empty()).then(|| filters.join(","))
    }

    /// 色彩空间与范围的转换。两项合成一条 zscale：分开写会让 ffmpeg 多解一遍。
    fn color_filters(&self) -> Option<String> {
        let mut args: Vec<&str> = Vec::new();
        if let Some(matrix) = self.color_matrix.zscale() {
            args.push(matrix);
        }
        if let Some(range) = self.color_range.zscale() {
            args.push(range);
        }
        (!args.is_empty()).then(|| format!("zscale={}", args.join(":")))
    }

    /// 两遍响度能不能用：得真的重编码音频，并且开着标准化。
    pub fn wants_two_pass_loudness(&self) -> bool {
        self.two_pass_loudness && self.normalize_audio && self.audio_filters_available()
    }

    /// 音频滤镜（单遍响度）。
    ///
    /// 音频偏移用「延迟」与「裁掉开头」两种写法实现：`adelay` 只推迟音频，
    /// 不碰画面；负数则把开头剪掉，等价于让声音提前。
    pub fn audio_filters(&self) -> Option<String> {
        self.audio_filters_with(None)
    }

    /// 输出时长。
    ///
    /// 开头定格会多出来一截：`-t` 与进度分母都必须按加长后的算，否则定格那段
    /// 会被 `-t` 直接截掉，进度条也会提前走满。
    pub fn output_duration(&self, range_duration: f64) -> f64 {
        let freeze = if self.filters_available() {
            self.freeze_seconds() as f64
        } else {
            0.0
        };
        range_duration.max(0.0) + freeze
    }

    /// 定格时长：非数字、负数与过大的值都收进 0–[`MAX_FREEZE_SECONDS`]。
    pub fn freeze_seconds(&self) -> f32 {
        if self.freeze_lead.is_finite() {
            self.freeze_lead.clamp(0.0, MAX_FREEZE_SECONDS)
        } else {
            0.0
        }
    }

    /// 音频滤镜。`measured` 有值时用两遍扫描的测量值做线性增益。
    pub fn audio_filters_with(&self, measured: Option<&Loudness>) -> Option<String> {
        if !self.audio_filters_available() {
            return None;
        }
        let mut filters: Vec<String> = Vec::new();
        if self.sync_audio {
            // 按音频包时间戳补齐/丢弃样本，长时间录制里声卡与视频时钟的微小
            // 偏差不会越积越多。first_pts=0 让第一段从 0 开始，避免整轨前移。
            filters.push("aresample=async=1:first_pts=0".to_string());
        }
        // 倒放与定格都改变时间轴，排在偏移之前：偏移是「相对最终画面」的位移，
        // 反过来排会让偏移量作用在还没重排的声音上。
        if self.reverse {
            filters.push("areverse".to_string());
        }
        if self.freeze_seconds() > 0.0 {
            filters.push(format!(
                "adelay={}:all=1",
                (self.freeze_seconds() * 1000.0).round() as i64
            ));
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
            filters.push(match measured {
                Some(loudness) => loudness.filter(),
                None => format!("loudnorm={}", super::loudness::TARGETS),
            });
        }
        // 增益排在 loudnorm 后面：反过来的话标准化会把刚加上去的几 dB 又拉回目标值，
        // 用户看到的是「调了没反应」。
        if self.volume_db.abs() > 0.05 {
            filters.push(format!("volume={:.1}dB", self.volume_db));
        }
        (!filters.is_empty()).then(|| filters.join(","))
    }
}

/// 固定码率的默认值：6000 kbps 是 1080p 上传最常见的档位，画质够看、体积可控。
pub const DEFAULT_BITRATE_KBPS: u32 = 6_000;

/// 开头定格的上限。再长就不该用「裁切」来做，而是去剪辑软件里排时间轴了。
pub const MAX_FREEZE_SECONDS: f32 = 10.0;

/// GIF 的输出链：限帧率、缩到 480 宽（只缩不放）、再用两遍法生成调色板。
///
/// `split` 把同一帧同时喂给「统计调色板」与「套用调色板」两条链，颜色比默认的
/// 固定调色板干净得多，代价是多解一遍。这段必须排在最后——调色板一旦生成，
/// 后面就只能画 256 色的图了，烧录字幕得在它之前。
pub(super) const GIF_FILTERS: &str = "fps=12,scale=min(480\\,iw):-2:flags=lanczos,\
     split[s0][s1];[s0]palettegen[p];[s1][p]paletteuse";

/// 把临时字幕文件写成 `subtitles` 滤镜参数。
///
/// ffmpeg 的滤镜串用 `:` 和 `,` 分隔参数，Windows 路径里的盘符冒号必须转义，
/// 反斜杠统一换成正斜杠；再整体包进单引号，路径里的空格与括号就不用再管。
fn subtitle_filter(path: &Path) -> String {
    format!("subtitles=filename='{}'", effects::filter_path(path))
}

/// 拼 ffmpeg 参数。
///
/// `burn` 是已经抽好的字幕文件；有值时把它烧进画面，并且不再复制内置字幕轨。
///
/// `-ss` 放在 `-i` 前：重编码时 ffmpeg 会先跳到目标附近再精确解码到目标帧，
/// 起点精确到帧，长视频也不用从第 0 秒解起；流复制时这一跳只能落在关键帧上，
/// 这也是「无损」必须接受的偏差。
pub(super) fn build_command(
    executable: &Path,
    request: &ClipRequest,
    encoder: &str,
    burn: Option<&Path>,
) -> Command {
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
        // 开头定格已经算进输出时长：`-t` 按原区间写会把定格那段截掉。
        .arg(format!(
            "{:.3}",
            options.output_duration(request.range.duration())
        ))
        // 大写 V 排除封面图（attached_pic），否则第一路「视频」可能是一张 jpeg。
        .arg("-map")
        .arg("0:V:0?");
    // HLS 的 TS 分片装不下字幕；GIF 干脆没有音轨与字幕，就算调用方没清掉开关，
    // 这里也不再映射。
    let hls = options.format == OutputFormat::Hls;
    let gif = options.format == OutputFormat::Gif;
    if options.keep_audio && !gif {
        command.arg("-map").arg("0:a?");
    }
    // 烧录之后画面里已经有字幕了，再复制一份软字幕只会让播放器叠两遍。
    let burning = burn.is_some();
    if options.keep_subtitles && !hls && !gif && !burning {
        command.arg("-map").arg("0:s?");
    }
    command
        .arg("-map_metadata")
        .arg(if hls || gif { "-1" } else { "0" })
        .arg("-map_chapters")
        .arg(if !hls && !gif && options.keep_chapters {
            "0"
        } else {
            "-1"
        });

    if options.mode == ExportMode::Copy && !gif {
        // 画、音、字幕一起复制：这时候任何 `-c:v` 之类都会把「无损」两个字作废。
        command.arg("-c").arg("copy");
    } else {
        if gif {
            // GIF 有自己的编码器，没有 CRF / 预设可调；调色板与帧率都在 `-vf` 里。
            command.arg("-c:v").arg("gif");
        } else {
            command.arg("-c:v").arg(encoder);
            for arg in super::encoder::video_args(encoder, options) {
                command.arg(arg);
            }
            // 默认不写 `-pix_fmt`：省略它，ffmpeg 会在「源格式 ∩ 编码器支持列表」里
            // 挑最接近的一个，10bit 源因此能保住 10bit。三种情况必须写死 yuv420p：
            // 用户主动要求兼容性、硬件编码器（多数型号只吃 8bit 4:2:0，不指定会初始化
            // 失败）、HLS（TS 分片里的 10bit H.264 在老设备和浏览器上支持面太窄）。
            let force_8bit = options.pixel_format == PixelFormat::Yuv420p
                || hls
                || crate::encoder::is_hardware(encoder);
            if force_8bit {
                command.arg("-pix_fmt").arg("yuv420p");
            }
        }
        // 滤镜链的顺序是有讲究的：先做几何与画面处理，再把字幕烧上去（按最终
        // 画面大小排版），GIF 的调色板必须排最后，否则字幕也会被压进 256 色里。
        let mut filters: Vec<String> = Vec::new();
        if let Some(base) = options.video_filters() {
            filters.push(base);
        }
        if let Some(path) = burn {
            filters.push(subtitle_filter(path));
        }
        if gif {
            filters.push(GIF_FILTERS.to_string());
        }
        if !filters.is_empty() {
            command.arg("-vf").arg(filters.join(","));
        }
        if options.force_cfr && !gif {
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
        if options.tag_rec709 && !gif {
            // 只改写容器标签、不动像素：专门修「标签写错导致播放器偏色」的源。
            command
                .arg("-color_primaries")
                .arg("bt709")
                .arg("-color_trc")
                .arg("bt709")
                .arg("-colorspace")
                .arg("bt709");
        }
        if options.keep_audio && !gif {
            if options.copies_audio() {
                command.arg("-c:a").arg("copy");
            } else {
                command.arg("-c:a").arg("aac").arg("-b:a").arg("192k");
                let filters = options.audio_filters_with(request.loudness.as_ref());
                if let Some(filters) = &filters {
                    command.arg("-af").arg(filters);
                }
                // loudnorm 内部工作在 192kHz，挂了滤镜却不指定采样率时输出会被它
                // 顶上去；用户显式选的采样率优先。
                match (options.sample_rate, filters.is_some()) {
                    (Some(rate), _) => {
                        command.arg("-ar").arg(rate.to_string());
                    }
                    (None, true) => {
                        command.arg("-ar").arg("48000");
                    }
                    (None, false) => {}
                }
                if let Some(channels) = options.channels {
                    command.arg("-ac").arg(channels.to_string());
                }
            }
        }
        if options.keep_subtitles && !hls && !gif && !burning {
            command.arg("-c:s").arg("copy");
        }
    }

    command.arg("-avoid_negative_ts").arg("make_zero");
    if gif {
        // 无限循环。不写这一句，多数播放器只放一遍就停。
        command.arg("-loop").arg("0");
    }
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
