//! 把序列展开成 ffmpeg 参数。纯字符串，不碰进程。
//!
//! 时间线的空档不能省：concat 只认「首尾相接的一串片段」，中间少一段，后面所有
//! 画面都会提前。视频空档补黑场（`color`），音频空档补静音（`anullsrc`）。
//!
//! 多轨规则与预览一致（M4）：每条启用轨道先各自拼成一条**和序列等长**的流，
//! 视频自下而上 `overlay`，音频全部丢进 `amix`。等长是硬要求——`overlay` 默认
//! 按最短的一条截断，轨道没铺满序列的话，成片会莫名少掉尾巴。

use std::path::PathBuf;

use crate::timeline::{AssetId, Id, Project, Sequence, Tick, Track};

use super::{ExportError, ExportPlan, ExportSettings, SubtitleAttachment};

/// 音频统一到 48kHz 立体声。导出链里混采样率是最常见的「声音变调」来源。
const AUDIO_RATE: u32 = 48_000;

/// HDR → SDR 的转换链：线性化成浮点、换到 bt709 原色、再按 bt709 编码。
///
/// `tonemap=hable` 与预览侧下发的 `tone-mapping=hable` 是同一条曲线（ADR 0007）：
/// 两边曲线不一致的话，预览里调好的曝光导出来会变一个样。
const HDR_TO_SDR: &str = "zscale=t=linear:npl=100,format=gbrpf32le,\
zscale=p=bt709,tonemap=hable:desat=0,\
zscale=t=bt709:m=bt709:r=tv,format=yuv420p";

/// 生成整条序列的导出计划（默认编码参数）。
pub fn plan_sequence(project: &Project) -> Result<ExportPlan, ExportError> {
    plan_sequence_with(project, None, &ExportSettings::default())
}

/// 生成整条序列的导出计划，编码参数走一套自定义设置。
pub fn plan_sequence_range(
    project: &Project,
    range: Option<(Tick, Tick)>,
) -> Result<ExportPlan, ExportError> {
    plan_sequence_with(project, range, &ExportSettings::default())
}

/// 生成导出计划。`range` 有值时只导这一段（工作区入 / 出点）。
///
/// 做法是在拼好的成片上再切一刀，而不是逐段裁剪：滤镜图里的段、空档、转场都按
/// 序列时间算好了，最后 `trim` / `atrim` 一次既不会漏段，也不会把转场切坏——
/// 切点落在转场中间时就是半截转场，与「只导这一段」的直觉一致。
///
/// 代价是要从头解到入点：导一条长片尾部的几秒，前面那些片也得先解一遍。要优化
/// 再改成逐段裁剪（先把 `range` 传进 `video_stream` / `audio_stream`）。
///
/// 编码参数只影响 `output_args`：滤镜图与输入列表与设置无关，所以自定义设置不会
/// 改变成片的内容（画质与体积除外）。
pub fn plan_sequence_with(
    project: &Project,
    range: Option<(Tick, Tick)>,
    settings: &ExportSettings,
) -> Result<ExportPlan, ExportError> {
    plan_sequence_id(project, project.active_id(), range, settings)
}

/// 按序列 id 生成计划。
///
/// 导出面板打开时记下的是「当时那条序列」：文件选择框期间用户可能切走，甚至把
/// 那条序列删掉。按 id 取序列才不会把 B 序列导成 A 的成片（ADR 0009）。
pub fn plan_sequence_id(
    project: &Project,
    sequence_id: Id,
    range: Option<(Tick, Tick)>,
    settings: &ExportSettings,
) -> Result<ExportPlan, ExportError> {
    let sequence = project
        .sequence_by_id(sequence_id)
        .ok_or(ExportError::SequenceGone)?;
    build_plan(project, sequence, range, settings)
}

fn build_plan(
    project: &Project,
    sequence: &Sequence,
    range: Option<(Tick, Tick)>,
    settings: &ExportSettings,
) -> Result<ExportPlan, ExportError> {
    if sequence.tracks.iter().all(|track| track.clips.is_empty()) {
        return Err(ExportError::Empty);
    }

    // 轨道顺序就是叠放顺序：列表靠上的画在上面。选轨规则与预览共用一份
    // （`timeline::preview::enabled_video_tracks`），两边不会各写一遍。
    let video_tracks: Vec<&Track> =
        crate::timeline::preview::enabled_video_tracks(sequence).collect();
    if video_tracks.is_empty() {
        return Err(ExportError::NoVideoTrack);
    }
    // 音频选轨同样与预览共用一份（`enabled_audio_tracks`）：静音 / 被独奏挤掉的轨
    // 都不进混音，两边的「哪些声音在响」因此不会各写一遍。
    let audio_tracks: Vec<&Track> =
        crate::timeline::preview::enabled_audio_tracks(sequence).collect();

    let duration = sequence.duration();
    // 工作区先夹到序列范围内；夹完是空的就没得导。
    let range = match range {
        Some((start, end)) => {
            let start = start.clamp(Tick::ZERO, duration);
            let end = end.clamp(Tick::ZERO, duration);
            if end <= start {
                return Err(ExportError::EmptyRange);
            }
            Some((start, end))
        }
        None => None,
    };
    let mut inputs = Vec::new();
    let mut video_layers = Vec::new();
    for track in &video_tracks {
        video_layers.push(video_stream(
            project,
            track,
            sequence,
            duration,
            &mut inputs,
        )?);
    }
    let mut audio_layers = Vec::new();
    for track in &audio_tracks {
        audio_layers.push(audio_stream(project, track, duration, &mut inputs)?);
    }

    let mut parts: Vec<String> = Vec::new();
    let mut video_label = Some("[outv]".to_string());

    // 视频：一层直接出 `[outv]`，多层自下而上叠。
    let layer_count = video_layers.len();
    let mut layer_labels = Vec::new();
    for (index, layer) in video_layers.into_iter().enumerate() {
        let label = if layer_count == 1 {
            "outv".to_string()
        } else {
            format!("layerv{index}")
        };
        // 先算 concat 再移走 `chains`：`concat` 还要读 `segments`。
        let concat = layer.concat(&label, true).expect("每条轨至少有一段");
        parts.extend(layer.chains);
        parts.push(concat);
        layer_labels.push(format!("[{label}]"));
    }
    if layer_count > 1 {
        // 最底层是列表里的最后一条，往上依次 overlay，最后一次落到 `[outv]`。
        let mut current = layer_labels.last().cloned().expect("至少两层");
        for index in (0..layer_labels.len() - 1).rev() {
            let out = if index == 0 {
                "[outv]".to_string()
            } else {
                format!("[ov{index}]")
            };
            parts.push(format!(
                "{current}{}overlay=0:0:format=auto{out}",
                layer_labels[index]
            ));
            current = out;
        }
    }

    let mut audio_label = if audio_layers.is_empty() {
        None
    } else {
        let count = audio_layers.len();
        let mut labels = Vec::new();
        for (index, layer) in audio_layers.into_iter().enumerate() {
            let label = if count == 1 {
                "outa".to_string()
            } else {
                format!("layera{index}")
            };
            let concat = layer.concat(&label, false).expect("每条轨至少有一段");
            parts.extend(layer.chains);
            parts.push(concat);
            labels.push(format!("[{label}]"));
        }
        if count > 1 {
            // `amix` 默认把总和归一（normalize=1）：两条满音量的轨混在一起也不会削顶。
            parts.push(format!(
                "{}amix=inputs={count}:duration=longest:dropout_transition=0[outa]",
                labels.concat()
            ));
        }
        Some("[outa]".to_string())
    };

    // 只导工作区：在拼好的成片上切一刀，重新从 0 起算。
    if let Some((start, end)) = range {
        let (start, end) = (start.secs(), end.secs());
        parts.push(format!(
            "[outv]trim=start={start:.6}:end={end:.6},setpts=PTS-STARTPTS[rangev]"
        ));
        video_label = Some("[rangev]".to_string());
        if let Some(label) = audio_label.take() {
            parts.push(format!(
                "{label}atrim=start={start:.6}:end={end:.6},asetpts=PTS-STARTPTS[rangea]"
            ));
            audio_label = Some("[rangea]".to_string());
        }
    }

    let output_args = settings.output_args(audio_label.is_some());
    // 字幕 ASS 文本按同一段范围裁切 / 平移，成片从 0 起算字幕也跟着从 0 起算。
    // 隐藏的字幕轨和落在工作区外的条目都不进脚本；一条都不剩就不附字幕。
    let subtitle =
        crate::timeline::subtitle::ass_script(sequence, range).map(|ass| SubtitleAttachment {
            ass,
            mode: settings.subtitle,
        });

    Ok(ExportPlan {
        inputs,
        filter: parts.join(";"),
        video_label,
        audio_label,
        output_args,
        subtitle,
    })
}

/// 一条流（视频或音频）的拼接进度：每段一条滤镜链、一个标签。
struct Stream {
    chains: Vec<String>,
    segments: Vec<String>,
}

impl Stream {
    fn new() -> Self {
        Self {
            chains: Vec::new(),
            segments: Vec::new(),
        }
    }

    fn push(&mut self, chain: String, label: String) {
        // 输出标签属于这条链的尾巴，拼在链上而不是只记在 segments 里。
        self.chains.push(format!("{chain}{label}"));
        self.segments.push(label);
    }

    /// 拼出 `concat` 滤镜；没有段就没有输出标签。
    fn concat(&self, out: &str, video: bool) -> Option<String> {
        if self.segments.is_empty() {
            return None;
        }
        let mapping = if video { "v=1:a=0" } else { "v=0:a=1" };
        Some(format!(
            "{}concat=n={}:{mapping}[{out}]",
            self.segments.concat(),
            self.segments.len()
        ))
    }
}

/// 一道要拼进 concat 的画面：一条或几条滤镜链，加上最终输出标签。
struct Piece {
    /// 按顺序发出去的链，**每条链尾都带着自己的输出标签**。合并了转场时，
    /// 前面几段的链也在这里面；最后一条链（xfade）的标签就是 [`Piece::label`]。
    chains: Vec<String>,
    /// 最终输出标签，进 concat 的就是它。
    label: String,
    start: Tick,
    end: Tick,
    /// 空档为 `None`：转场只认片段，不认黑场。
    clip: Option<crate::timeline::Id>,
}

impl Piece {
    fn len(&self) -> Tick {
        self.end - self.start
    }
}

fn video_stream(
    project: &Project,
    track: &Track,
    sequence: &Sequence,
    duration: Tick,
    inputs: &mut Vec<PathBuf>,
) -> Result<Stream, ExportError> {
    let mut cursor = Tick::ZERO;
    let mut gap_index = 0;
    // `color` 的尺寸必须写成 `WxH`：滤镜参数用 `:` 分隔，`W:H` 会被当成两个参数。
    // `scale` / `pad` 反过来只能用 `W:H`（它们是位置参数 w/h/x/y）。
    let color_size = format!("{}x{}", sequence.width, sequence.height);
    let dims = format!("{}:{}", sequence.width, sequence.height);
    let fps = format!("{}/{}", sequence.fps.num(), sequence.fps.den());
    let mut pieces: Vec<Piece> = Vec::new();

    for clip in &track.clips {
        // 空档补黑：不补的话 concat 会把后面的画面整体提前。
        if clip.start > cursor {
            // 带上轨道 id：多条轨都有空档时，各轨的 gap_index 都从 0 开始，
            // 不带轨道会产出两个同名 `[bgv0]`，ffmpeg 会拒绝重复的标签。
            let label = format!("[bgv{}_{gap_index}]", track.id.raw());
            pieces.push(Piece {
                chains: vec![format!(
                    "color=c=black:s={color_size}:r={fps}:d={:.6}{label}",
                    (clip.start - cursor).secs()
                )],
                label,
                start: cursor,
                end: clip.start,
                clip: None,
            });
            gap_index += 1;
        }

        let index = input_index(inputs, project, clip.asset_id)?;

        // 交叉溶解要多取一段前一段的素材（往后延）和本片段之前的素材（往前挪）。
        // 前一段没有余量就退化成硬切——宁可切得生硬，也不要 ffmpeg 因缺帧把时长算错。
        let mut head = Tick::ZERO;
        let mut kind = None;
        // 变速片段不接转场：`xfade` 的时长按时间线算，而多取的素材按素材时间算，
        // 两头一换算就容易错帧。宁可退化成硬切，也不要成片时长对不上。
        let rate = clip.rate();
        if let Some(previous) = pieces.last()
            && previous.end == clip.start
            && (rate - 1.0).abs() < 1e-6
            && let Some(previous_id) = previous.clip
            && let Some(transition) = track
                .transitions
                .iter()
                .find(|transition| transition.before == previous_id && transition.after == clip.id)
        {
            let usable = transition.duration.min(previous.len()).min(clip.source_in);
            if usable > Tick::ZERO {
                head = usable;
                kind = Some(transition.kind);
            }
        }

        // HDR 素材先转成 SDR 再缩放：成片的色彩空间固定是 bt709（见 ADR 0007），
        // 混进一段没做 tonemap 的 HLG 画面，整条片子都会发灰。
        let tonemap = if asset_is_hdr(project, clip.asset_id) {
            format!(",{HDR_TO_SDR}")
        } else {
            String::new()
        };
        // 视频必须用 `trim` + `setpts`：`atrim` 是音频滤镜，它的输出 pad 是 audio，
        // 接到 `setpts` 的 video 输入上会让整张 filter_complex 图连不起来。
        // 效果链排在缩放补边之后、转场之前：构图先定下来，降噪 / 锐化 / 水印
        // 都作用在成片尺寸上，字号与锐化强度才不会随素材分辨率乱跳。
        let effects = crate::timeline::effect_chain(sequence, clip.id);
        // 关键帧排在最前（缩放 / 位置 → 不透明度 → 效果）：先把画面构图推到该有的
        // 样子，再压噪点、再盖水印，水印不会跟着推近一起放大。
        let motion = match crate::timeline::keyframe_motion(&clip.keys, sequence) {
            filter if filter.is_empty() => String::new(),
            filter => format!(",{filter}"),
        };
        // 调色排在构图之后、淡入淡出之前：先定画面尺寸与位移，再统一色彩，
        // 最后才做不透明度。水印在 `effects` 里，排在后面，不会被调色一起染上。
        let color = match clip.color.filter_chain(project.base_dir()) {
            chain if chain.is_empty() => String::new(),
            chain => format!(",{chain}"),
        };
        let fades = crate::timeline::keyframe_fades(&clip.keys, clip.duration())
            .iter()
            .map(|filter| format!(",{filter}"))
            .collect::<String>();
        let label = format!("[v{}]", clip.id.raw());
        // 输出标签必须拼在链尾：concat 与 xfade 只认标签，链上没有标签时
        // ffmpeg 会报 "Stream specifier 'v3' ... matches no streams"。
        // 变速：`setpts` 按倍率压缩 / 拉伸时间戳，音轨那边用 `atempo` 对齐。
        let setpts = if (rate - 1.0).abs() < 1e-6 {
            "setpts=PTS-STARTPTS".to_string()
        } else {
            format!("setpts=(PTS-STARTPTS)/{rate:.6}")
        };
        // 片尾定格：克隆最后一帧停 `hold` 秒，音轨补等长静音。
        let freeze = if clip.hold > Tick::ZERO {
            format!(
                ",tpad=stop_mode=clone:stop_duration={:.6}",
                clip.hold.secs()
            )
        } else {
            String::new()
        };
        let chain = format!(
            "[{index}:v]trim=start={:.6}:end={:.6},{setpts}{freeze},\
             scale={dims}:force_original_aspect_ratio=decrease,\
             pad={dims}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={fps}{tonemap}{motion}{color}{fades}{effects}{label}",
            (clip.source_in - head).secs(),
            clip.source_out.secs()
        );

        match kind {
            Some(kind) => {
                let previous = pieces.pop().expect("有转场就一定有前一段");
                // `xfade` 从 `offset` 开始溶到本片段：offset 落在前一段的尾巴上，
                // 也就是「前一段总长 - 溶接时长」。两段加起来仍等于原时长。
                // 两个 label 都自带方括号，直接相连：`[[v4]]` 会被 ffmpeg 当成
                // 一个不存在的滤镜名（报 "No such filter: ']xfade'"）。
                let xfade = format!(
                    "{}{label}xfade=transition={}:duration={:.6}:offset={:.6}[xf{}]",
                    previous.label,
                    xfade_name(kind),
                    head.secs(),
                    (previous.len() - head).secs().max(0.0),
                    clip.id.raw()
                );
                let mut chains = previous.chains;
                chains.push(chain);
                chains.push(xfade);
                pieces.push(Piece {
                    chains,
                    label: format!("[xf{}]", clip.id.raw()),
                    start: previous.start,
                    end: clip.end(),
                    clip: Some(clip.id),
                });
            }
            None => pieces.push(Piece {
                chains: vec![chain],
                label,
                start: clip.start,
                end: clip.end(),
                clip: Some(clip.id),
            }),
        }
        cursor = clip.end();
    }

    // 这条轨没铺满序列：补齐到整条序列的长度。多层 overlay 要求等长，
    // 而 `overlay` 默认按最短的一条截断——少这一段的成片会缺尾巴。
    if cursor < duration {
        let label = format!("[bgv{}_{gap_index}]", track.id.raw());
        pieces.push(Piece {
            chains: vec![format!(
                "color=c=black:s={color_size}:r={fps}:d={:.6}{label}",
                (duration - cursor).secs()
            )],
            label,
            start: cursor,
            end: duration,
            clip: None,
        });
    }

    let mut stream = Stream::new();
    for piece in pieces {
        // 黑场、片段、xfade 的链都自带输出标签，和 audio_stream 的 push 一样；
        // 这里只收集，不能再拼一次，否则转场那条链会多出一个标签。
        stream.chains.extend(piece.chains);
        stream.segments.push(piece.label);
    }
    Ok(stream)
}

/// 倍率 → `atempo` 链。
///
/// `atempo` 一档只认 0.5–2.0，更极端的倍率要串联：4x 是 `atempo=2.0,atempo=2.0`，
/// 0.25x 是 `atempo=0.5,atempo=0.5`。串起来乘起来必须正好等于目标倍率。
fn atempo_chain(rate: f32) -> String {
    if (rate - 1.0).abs() < 1e-6 || !rate.is_finite() || rate <= 0.0 {
        return String::new();
    }
    let mut rest = f64::from(rate);
    let mut parts: Vec<String> = Vec::new();
    while rest > 2.0 {
        parts.push("atempo=2.0".to_string());
        rest /= 2.0;
    }
    while rest < 0.5 {
        parts.push("atempo=0.5".to_string());
        rest /= 0.5;
    }
    parts.push(format!("atempo={rest:.6}"));
    parts.iter().map(|part| format!(",{part}")).collect()
}

/// 转场类型 → `xfade` 的 `transition` 取值。
fn xfade_name(kind: crate::timeline::TransitionKind) -> &'static str {
    match kind {
        crate::timeline::TransitionKind::CrossDissolve => "dissolve",
        crate::timeline::TransitionKind::Fade => "fade",
        crate::timeline::TransitionKind::Wipe => "wipeleft",
        crate::timeline::TransitionKind::Slide => "slideleft",
    }
}

fn audio_stream(
    project: &Project,
    track: &Track,
    duration: Tick,
    inputs: &mut Vec<PathBuf>,
) -> Result<Stream, ExportError> {
    let mut stream = Stream::new();
    let mut cursor = Tick::ZERO;
    let mut gap_index = 0;
    // 轨道音量落成 `volume=`：它是振幅倍数，与预览侧乘到 mpv 音量上的那个数同量纲。
    // 静音段不用乘——`anullsrc` 多少倍都是静音，少插一道滤镜更清楚。
    let gain = track.normalized_gain();
    let volume = if (gain - 1.0).abs() < 1e-4 {
        String::new()
    } else {
        format!(",volume={gain:.4}")
    };

    for clip in &track.clips {
        if clip.start > cursor {
            let duration = (clip.start - cursor).secs();
            stream.push(
                format!(
                    "anullsrc=r={AUDIO_RATE}:cl=stereo,\
                     atrim=duration={duration:.6}"
                ),
                // 同理：静音段标签也要带轨道 id，避免多轨同名。
                format!("[bga{}_{gap_index}]", track.id.raw()),
            );
            gap_index += 1;
        }

        let index = input_index(inputs, project, clip.asset_id)?;
        let label = format!("[a{}]", clip.id.raw());
        // 变速片段的声音跟着改速率；定格段补静音，好和视频那边的 `tpad` 对齐。
        let tempo = atempo_chain(clip.rate());
        let pad = if clip.hold > Tick::ZERO {
            format!(",apad=pad_dur={:.6}", clip.hold.secs())
        } else {
            String::new()
        };
        // 三层音量（片段 → 自动化 → 推子）连乘，顺序见 `timeline::mixer`；
        // 连乘与顺序无关，分开写只是为了每一层都能单独看出它在哪儿。
        let clip_gain = crate::timeline::mixer::clip_gain(clip.gain);
        let clip_volume = if (clip_gain - 1.0).abs() < 1e-4 {
            String::new()
        } else {
            format!(",volume={clip_gain:.4}")
        };
        // 自动化用 `eval=frame`：每帧按轨道时间求值。滤镜链里这一段从 PTS 0 开始，
        // 所以表达式里把片段起点加回去。
        let automation = crate::timeline::mixer::gain_expression(track, clip.start)
            .map(|expr| format!(",volume=volume='{expr}':eval=frame"))
            .unwrap_or_default();
        // 声像走 `pan`：左右各乘一个系数（等功率），居中时不插这道。
        let (left, right) = crate::timeline::mixer::pan_gains(track.pan);
        let pan = if (left - right).abs() < 1e-4 {
            String::new()
        } else {
            format!(",pan=stereo|c0={left:.4}*c0|c1={right:.4}*c1")
        };
        stream.push(
            format!(
                "[{index}:a]atrim=start={:.6}:end={:.6},asetpts=PTS-STARTPTS{tempo},\
                 aresample={AUDIO_RATE},\
                 aformat=sample_fmts=fltp:channel_layouts=stereo{pad}\
                 {clip_volume}{automation}{volume}{pan}",
                clip.source_in.secs(),
                clip.source_out.secs()
            ),
            label,
        );
        cursor = clip.end();
    }

    // 跟视频一样补齐到整条序列：`amix` 的 duration=longest 才不会把别的轨
    // 提前砍掉，各条轨也都在同一个时间零点上。
    if cursor < duration {
        stream.push(
            format!(
                "anullsrc=r={AUDIO_RATE}:cl=stereo,atrim=duration={:.6}",
                (duration - cursor).secs()
            ),
            format!("[bga{}_{gap_index}]", track.id.raw()),
        );
    }

    Ok(stream)
}

/// 这段素材是不是 HDR。没探测过（老工程、音频文件）按 SDR 处理：
/// 多插一条 tonemap 链要多一道浮点转换，猜错比漏做更贵。
fn asset_is_hdr(project: &Project, asset_id: AssetId) -> bool {
    project
        .asset(asset_id)
        .and_then(|asset| asset.media.as_ref())
        .is_some_and(|media| media.color.is_hdr())
}

/// 素材文件在 ffmpeg 输入里的下标；同一个文件只加一次 `-i`。
fn input_index(
    inputs: &mut Vec<PathBuf>,
    project: &Project,
    asset_id: AssetId,
) -> Result<usize, ExportError> {
    let asset = project
        .asset(asset_id)
        .ok_or(ExportError::UnknownAsset(asset_id.raw()))?;
    if asset.missing {
        return Err(ExportError::MissingAsset(asset.display_name().to_string()));
    }
    let path = project
        .asset_path(asset_id)
        .ok_or(ExportError::UnknownAsset(asset_id.raw()))?
        .to_path_buf();

    // 代理只是预览用的低码率副本：宁可在这里拒绝，也不要导出一个 480p 的成片。
    if crate::timeline::is_proxy_file(&path) {
        return Err(ExportError::ProxySource(asset.display_name().to_string()));
    }

    if let Some(index) = inputs.iter().position(|existing| existing == &path) {
        return Ok(index);
    }
    inputs.push(path);
    Ok(inputs.len() - 1)
}
