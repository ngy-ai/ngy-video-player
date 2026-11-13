//! 把序列展开成 ffmpeg 参数。纯字符串，不碰进程。
//!
//! 时间线的空档不能省：concat 只认「首尾相接的一串片段」，中间少一段，后面所有
//! 画面都会提前。视频空档补黑场（`color`），音频空档补静音（`anullsrc`）。
//!
//! 多轨规则与预览一致（M4）：每条启用轨道先各自拼成一条**和序列等长**的流，
//! 视频自下而上 `overlay`，音频全部丢进 `amix`。等长是硬要求——`overlay` 默认
//! 按最短的一条截断，轨道没铺满序列的话，成片会莫名少掉尾巴。

//! 本文件只做「组装」：拼输入表、按范围切一刀、把几条流接起来。
//! 单条流的滤镜链分别在 `plan/video.rs`（含遮罩的 alphamerge）与 `plan/audio.rs`
//! （含速度曲线的分段 atempo）里。

mod audio;
mod video;

use std::path::PathBuf;

pub(super) use audio::audio_stream;
use video::video_stream;

use super::MaskAttachment;
use crate::timeline::{AssetId, Id, Project, Sequence, Tick, Track};

use super::{ExportError, ExportPlan, ExportSettings, SubtitleAttachment};

/// 音频统一到 48kHz 立体声。导出链里混采样率是最常见的「声音变调」来源。
pub(super) const AUDIO_RATE: u32 = 48_000;

/// HDR → SDR 的转换链：线性化成浮点、换到 bt709 原色、再按 bt709 编码。
///
/// `tonemap=hable` 与预览侧下发的 `tone-mapping=hable` 是同一条曲线（ADR 0007）：
/// 两边曲线不一致的话，预览里调好的曝光导出来会变一个样。
pub(super) const HDR_TO_SDR: &str = "zscale=t=linear:npl=100,format=gbrpf32le,\
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
    let mut masks: Vec<MaskAttachment> = Vec::new();
    let mut video_layers = Vec::new();
    for track in &video_tracks {
        video_layers.push(video_stream(
            project,
            track,
            sequence,
            duration,
            &mut inputs,
            &mut masks,
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
    // 有没有遮罩：有就得从**黑底**开始叠。遮罩把画面切成透明的地方要露出下面的
    // 轨道；最底下那条轨没有「下面」，必须自己垫一层黑，否则透明区会留着原画面的
    // 颜色（`alphamerge` 只改 alpha，不改 RGB）。
    let masked = video_layers.iter().any(|layer| layer.masked);
    let mut layer_labels = Vec::new();
    for (index, layer) in video_layers.into_iter().enumerate() {
        let label = if layer_count == 1 && !masked {
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
    if layer_count > 1 || masked {
        // 最底层是列表里的最后一条，往上依次 overlay，最后一次落到 `[outv]`。
        // 有遮罩时改成从一块黑底开始叠：遮罩切出来的透明区要落到黑上，
        // 而不是露出原画面的颜色（`alphamerge` 只改 alpha，不改 RGB）。
        let mut current = if masked {
            parts.push(format!(
                "color=c=black:s={}x{}:r={}/{}:d={:.6}[maskbase]",
                sequence.width,
                sequence.height,
                sequence.fps.num(),
                sequence.fps.den(),
                duration.secs()
            ));
            "[maskbase]".to_string()
        } else {
            layer_labels.last().cloned().expect("至少两层")
        };
        let top = if masked {
            layer_labels.len()
        } else {
            layer_labels.len() - 1
        };
        for index in (0..top).rev() {
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
        masks,
    })
}

/// 一条流（视频或音频）的拼接进度：每段一条滤镜链、一个标签。
pub(super) struct Stream {
    pub(super) chains: Vec<String>,
    pub(super) segments: Vec<String>,
    /// 这条轨上有没有遮罩：有的话整条轨是 yuva420p，叠之前要先垫黑底。
    masked: bool,
}

impl Stream {
    fn new() -> Self {
        Self {
            chains: Vec::new(),
            segments: Vec::new(),
            masked: false,
        }
    }

    fn push(&mut self, chain: String, label: String) {
        // 输出标签属于这条链的尾巴，拼在链上而不是只记在 segments 里。
        self.chains.push(format!("{chain}{label}"));
        self.segments.push(label);
    }

    /// 一段要好几条滤镜链的素材（速度曲线的分段变速就是这样）：中间那几条各自带
    /// 自己的标签，只有最后一条落在 `label` 上，`segments` 里也只记一次。
    fn push_chains(&mut self, chains: Vec<String>, label: String) {
        self.chains.extend(chains);
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
