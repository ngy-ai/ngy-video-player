//! 把序列展开成 ffmpeg 参数。纯字符串，不碰进程。
//!
//! 时间线的空档不能省：concat 只认「首尾相接的一串片段」，中间少一段，后面所有
//! 画面都会提前。视频空档补黑场（`color`），音频空档补静音（`anullsrc`）。
//!
//! 多轨规则与预览一致（M4）：每条启用轨道先各自拼成一条**和序列等长**的流，
//! 视频自下而上 `overlay`，音频全部丢进 `amix`。等长是硬要求——`overlay` 默认
//! 按最短的一条截断，轨道没铺满序列的话，成片会莫名少掉尾巴。

use std::path::PathBuf;

use crate::timeline::{AssetId, Project, Sequence, Tick, Track, TrackKind};

use super::{ExportError, ExportPlan};

/// 音频统一到 48kHz 立体声。导出链里混采样率是最常见的「声音变调」来源。
const AUDIO_RATE: u32 = 48_000;

/// HDR → SDR 的转换链：线性化成浮点、换到 bt709 原色、再按 bt709 编码。
///
/// `tonemap=hable` 与预览侧下发的 `tone-mapping=hable` 是同一条曲线（ADR 0007）：
/// 两边曲线不一致的话，预览里调好的曝光导出来会变一个样。
const HDR_TO_SDR: &str = "zscale=t=linear:npl=100,format=gbrpf32le,\
zscale=p=bt709,tonemap=hable:desat=0,\
zscale=t=bt709:m=bt709:r=tv,format=yuv420p";

/// 生成整条序列的导出计划。
pub fn plan_sequence(project: &Project) -> Result<ExportPlan, ExportError> {
    let sequence = &project.sequence;
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
    let audio_tracks: Vec<&Track> = sequence
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Audio && !track.muted && !track.clips.is_empty())
        .collect();

    let duration = sequence.duration();
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
    let video_label = Some("[outv]".to_string());

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

    let audio_label = if audio_layers.is_empty() {
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

    let mut output_args: Vec<String> = vec![
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "medium".into(),
        "-crf".into(),
        "18".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
    ];
    if audio_label.is_some() {
        output_args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "192k".into()]);
    } else {
        output_args.push("-an".into());
    }
    output_args.extend(["-movflags".into(), "+faststart".into()]);

    Ok(ExportPlan {
        inputs,
        filter: parts.join(";"),
        video_label,
        audio_label,
        output_args,
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
        if let Some(previous) = pieces.last()
            && previous.end == clip.start
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
        let fades = crate::timeline::keyframe_fades(&clip.keys, clip.duration())
            .iter()
            .map(|filter| format!(",{filter}"))
            .collect::<String>();
        let label = format!("[v{}]", clip.id.raw());
        // 输出标签必须拼在链尾：concat 与 xfade 只认标签，链上没有标签时
        // ffmpeg 会报 "Stream specifier 'v3' ... matches no streams"。
        let chain = format!(
            "[{index}:v]trim=start={:.6}:end={:.6},setpts=PTS-STARTPTS,\
             scale={dims}:force_original_aspect_ratio=decrease,\
             pad={dims}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={fps}{tonemap}{motion}{fades}{effects}{label}",
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
        stream.push(
            format!(
                "[{index}:a]atrim=start={:.6}:end={:.6},asetpts=PTS-STARTPTS,\
                 aresample={AUDIO_RATE},\
                 aformat=sample_fmts=fltp:channel_layouts=stereo",
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

    if let Some(index) = inputs.iter().position(|existing| existing == &path) {
        return Ok(index);
    }
    inputs.push(path);
    Ok(inputs.len() - 1)
}
