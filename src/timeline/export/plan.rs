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

/// 生成整条序列的导出计划。
pub fn plan_sequence(project: &Project) -> Result<ExportPlan, ExportError> {
    let sequence = &project.sequence;
    if sequence.tracks.iter().all(|track| track.clips.is_empty()) {
        return Err(ExportError::Empty);
    }

    // 轨道顺序就是叠放顺序：列表靠上的画在上面。静音 / 隐藏的轨道整条跳过。
    let video_tracks: Vec<&Track> = sequence
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Video && !track.muted && !track.clips.is_empty())
        .collect();
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

fn video_stream(
    project: &Project,
    track: &Track,
    sequence: &Sequence,
    duration: Tick,
    inputs: &mut Vec<PathBuf>,
) -> Result<Stream, ExportError> {
    let mut stream = Stream::new();
    let mut cursor = Tick::ZERO;
    let mut gap_index = 0;
    // `color` 的尺寸必须写成 `WxH`：滤镜参数用 `:` 分隔，`W:H` 会被当成两个参数。
    // `scale` / `pad` 反过来只能用 `W:H`（它们是位置参数 w/h/x/y）。
    let color_size = format!("{}x{}", sequence.width, sequence.height);
    let dims = format!("{}:{}", sequence.width, sequence.height);
    let fps = format!("{}/{}", sequence.fps.num(), sequence.fps.den());

    for clip in &track.clips {
        // 空档补黑：不补的话 concat 会把后面的画面整体提前。
        if clip.start > cursor {
            let duration = (clip.start - cursor).secs();
            stream.push(
                format!("color=c=black:s={color_size}:r={fps}:d={duration:.6}"),
                format!("[bgv{gap_index}]"),
            );
            gap_index += 1;
        }

        let index = input_index(inputs, project, clip.asset_id)?;
        let label = format!("[v{}]", clip.id.raw());
        stream.push(
            format!(
                "[{index}:v]trim=start={:.6}:end={:.6},setpts=PTS-STARTPTS,\
                 scale={dims}:force_original_aspect_ratio=decrease,\
                 pad={dims}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={fps}",
                clip.source_in.secs(),
                clip.source_out.secs()
            ),
            label,
        );
        cursor = clip.end();
    }

    // 这条轨没铺满序列：补齐到整条序列的长度。多层 overlay 要求等长，
    // 而 `overlay` 默认按最短的一条截断——少这一段的成片会缺尾巴。
    if cursor < duration {
        stream.push(
            format!(
                "color=c=black:s={color_size}:r={fps}:d={:.6}",
                (duration - cursor).secs()
            ),
            format!("[bgv{gap_index}]"),
        );
    }

    Ok(stream)
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
                format!("[bga{gap_index}]"),
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
            format!("[bga{gap_index}]"),
        );
    }

    Ok(stream)
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
