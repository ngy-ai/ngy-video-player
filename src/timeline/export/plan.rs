//! 把序列展开成 ffmpeg 参数。纯字符串，不碰进程。
//!
//! 时间线的空档不能省：concat 只认「首尾相接的一串片段」，中间少一段，后面所有
//! 画面都会提前。视频空档补黑场（`color`），音频空档补静音（`anullsrc`）。

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

    // 第一版只取第一条启用的视频轨与第一条启用的音频轨，多轨叠加在 M3-T03/T04。
    let video_track = sequence
        .tracks
        .iter()
        .find(|track| track.kind == TrackKind::Video && !track.muted)
        .ok_or(ExportError::NoVideoTrack)?;

    let mut inputs = Vec::new();
    let video = video_stream(project, video_track, sequence, &mut inputs)?;
    if video.segments.is_empty() {
        return Err(ExportError::Empty);
    }
    let audio = sequence
        .tracks
        .iter()
        .find(|track| track.kind == TrackKind::Audio && !track.muted)
        .map(|track| audio_stream(project, track, &mut inputs))
        .transpose()?;

    // 先算 concat 再取 chains：`chains` 一被移走，`concat` 就借不到 `segments` 了。
    let video_concat = video.concat("outv", true).expect("视频段非空");
    let audio_concat = audio
        .as_ref()
        .filter(|audio| !audio.segments.is_empty())
        .and_then(|audio| audio.concat("outa", false));

    let mut parts = video.chains;
    parts.push(video_concat);
    let video_label = Some("[outv]".to_string());

    let audio_label = match audio {
        Some(audio) if !audio.segments.is_empty() => {
            parts.extend(audio.chains);
            parts.push(audio_concat.expect("音频段非空"));
            Some("[outa]".to_string())
        }
        _ => None,
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
    inputs: &mut Vec<PathBuf>,
) -> Result<Stream, ExportError> {
    let mut stream = Stream::new();
    let mut cursor = Tick::ZERO;
    let mut gap_index = 0;
    let size = format!("{}x{}", sequence.width, sequence.height);
    let fps = format!("{}/{}", sequence.fps.num(), sequence.fps.den());

    for clip in &track.clips {
        // 空档补黑：不补的话 concat 会把后面的画面整体提前。
        if clip.start > cursor {
            let duration = (clip.start - cursor).secs();
            stream.push(
                format!("color=c=black:s={size}:r={fps}:d={duration:.6}"),
                format!("[bgv{gap_index}]"),
            );
            gap_index += 1;
        }

        let index = input_index(inputs, project, clip.asset_id)?;
        let label = format!("[v{}]", clip.id.raw());
        stream.push(
            format!(
                "[{index}:v]trim=start={:.6}:end={:.6},setpts=PTS-STARTPTS,\
                 scale={size}:force_original_aspect_ratio=decrease,\
                 pad={size}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={fps}",
                clip.source_in.secs(),
                clip.source_out.secs()
            ),
            label,
        );
        cursor = clip.end();
    }

    Ok(stream)
}

fn audio_stream(
    project: &Project,
    track: &Track,
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
