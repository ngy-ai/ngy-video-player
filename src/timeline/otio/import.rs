//! OTIO JSON → 与工程无关的中间结构。
//!
//! 这里不碰工程、不碰文件系统：读出来的片段只带路径与时间，素材怎么登记、
//! 序列怎么建由界面层决定（见 `app/otio.rs`）。这样「读一份别人给的 OTIO」
//! 就能纯单测，不用起窗口、也不用真素材。
//!
//! 读不懂的东西一律降级而**不是**报错：不认识的轨道类型跳过、不认识的转场
//! 按交叉溶解、不认识的效果留成 `EffectKind::Unknown` 并在 `warnings` 里点名。
//! 用户拿到的是一条能打开的时间线，而不是「导入失败」四个字。

use super::*;
use crate::timeline::{
    ColorGrade, Effect, EffectKind, Keyframes, Rational, SpeedCurve, SubtitleItem, Tick, TrackKind,
    TransitionKind,
};
use serde_json::Value;
use std::path::PathBuf;

/// 读到的一条时间线。
#[derive(Debug)]
pub struct ImportedTimeline {
    /// 时间线名字（导入时当序列名）。
    pub name: String,
    /// `metadata.ngy` 里的帧率。对方文件没写就是 `None`，由界面沿用当前序列的规格。
    pub fps: Option<Rational>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub markers: Vec<(Tick, String)>,
    pub tracks: Vec<ImportedTrack>,
    /// 降级与跳过的地方，界面把它拼成一句提示。
    pub warnings: Vec<String>,
}

/// 读到的一条轨道。
#[derive(Debug)]
pub struct ImportedTrack {
    pub kind: TrackKind,
    pub name: String,
    pub muted: bool,
    pub locked: bool,
    pub gain: f32,
    pub solo: bool,
    pub clips: Vec<ImportedClip>,
    /// 转场：`after` 是它在 `clips` 里后面的那一段的序号。
    pub transitions: Vec<ImportedTransition>,
    pub subtitles: Vec<SubtitleItem>,
}

/// 读到的一个片段。
#[derive(Debug)]
pub struct ImportedClip {
    pub path: PathBuf,
    pub name: String,
    pub start: Tick,
    pub source_in: Tick,
    pub source_out: Tick,
    pub speed: f32,
    /// 速度曲线（时间重映射）。对方文件里没有就是空曲线，只按 `speed` 算。
    pub remap: SpeedCurve,
    pub hold: Tick,
    pub effects: Vec<Effect>,
    pub keys: Keyframes,
    pub color: ColorGrade,
}

/// 读到的转场。
#[derive(Debug)]
pub struct ImportedTransition {
    pub after: usize,
    pub kind: TransitionKind,
    pub duration: Tick,
}

/// 解析一份 `.otio` 文本。
pub fn import_timeline(text: &str) -> Result<ImportedTimeline, OtioError> {
    let root: Value =
        serde_json::from_str(text).map_err(|error| OtioError::Json(error.to_string()))?;
    let schema = root
        .get("OTIO_SCHEMA")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if !schema.starts_with("Timeline") {
        return Err(OtioError::NotTimeline(if schema.is_empty() {
            "（没有 OTIO_SCHEMA）".to_string()
        } else {
            schema
        }));
    }

    let meta = root.get("metadata").and_then(|meta| meta.get("ngy"));
    let mut warnings = Vec::new();
    let fps = meta
        .and_then(|meta| meta.get("fps"))
        .and_then(Value::as_str)
        .and_then(Rational::parse);
    let width = meta
        .and_then(|meta| meta.get("width"))
        .and_then(Value::as_u64)
        .map(|value| value as u32)
        .filter(|value| *value >= 2);
    let height = meta
        .and_then(|meta| meta.get("height"))
        .and_then(Value::as_u64)
        .map(|value| value as u32)
        .filter(|value| *value >= 2);
    let markers = meta
        .and_then(|meta| meta.get("markers"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let at = item.get("at").and_then(Value::as_i64)?;
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    Some((Tick::from_micros(at), name))
                })
                .collect()
        })
        .unwrap_or_default();

    let mut tracks = Vec::new();
    let children = root
        .get("tracks")
        .and_then(|stack| stack.get("children"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (index, child) in children.iter().enumerate() {
        let track_meta = child
            .get("metadata")
            .and_then(|meta| meta.get("ngy"))
            .cloned()
            .unwrap_or(Value::Null);
        match parse_track(child, &track_meta, &mut warnings) {
            Some(track) => tracks.push(track),
            // 空轨道（对方软件里的「参考轨」之类）直接不要：多一条空轨只会让
            // 用户以为自己导入了什么。
            None => {
                let name = child.get("name").and_then(Value::as_str).unwrap_or("?");
                warnings.push(format!(
                    "第 {} 条轨道「{name}」没有可用片段，已跳过",
                    index + 1
                ));
            }
        }
    }

    if let Some(items) = meta
        .and_then(|meta| meta.get("subtitle_tracks"))
        .and_then(Value::as_array)
    {
        for item in items {
            let mut track = ImportedTrack {
                kind: TrackKind::Subtitle,
                name: item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("字幕 1")
                    .to_string(),
                muted: item.get("muted").and_then(Value::as_bool).unwrap_or(false),
                locked: item.get("locked").and_then(Value::as_bool).unwrap_or(false),
                gain: 1.0,
                solo: false,
                clips: Vec::new(),
                transitions: Vec::new(),
                subtitles: Vec::new(),
            };
            if let Some(entries) = item.get("items").and_then(Value::as_array) {
                for entry in entries {
                    match serde_json::from_value::<SubtitleItem>(entry.clone()) {
                        Ok(mut parsed) => {
                            parsed.normalize();
                            track.subtitles.push(parsed);
                        }
                        Err(error) => warnings.push(format!("一条字幕读不出来：{error}")),
                    }
                }
            }
            tracks.push(track);
        }
    }

    if tracks.is_empty() {
        return Err(OtioError::NoTracks);
    }

    Ok(ImportedTimeline {
        name: root
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .unwrap_or("导入的序列")
            .to_string(),
        fps,
        width,
        height,
        markers,
        tracks,
        warnings,
    })
}

/// 一条轨道。没有可用片段时返回 `None`。
fn parse_track(value: &Value, meta: &Value, warnings: &mut Vec<String>) -> Option<ImportedTrack> {
    let kind_name = value.get("kind").and_then(Value::as_str).unwrap_or("Video");
    let kind = match kind_name {
        "Video" => TrackKind::Video,
        "Audio" => TrackKind::Audio,
        other => {
            warnings.push(format!("轨道类型 {other} 不支持，已跳过整条轨道"));
            return None;
        }
    };

    let mut track = ImportedTrack {
        kind,
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(kind.label())
            .to_string(),
        muted: meta.get("muted").and_then(Value::as_bool).unwrap_or(false),
        locked: meta.get("locked").and_then(Value::as_bool).unwrap_or(false),
        gain: meta
            .get("gain")
            .and_then(Value::as_f64)
            .map(|gain| gain as f32)
            .unwrap_or(1.0),
        solo: meta.get("solo").and_then(Value::as_bool).unwrap_or(false),
        clips: Vec::new(),
        transitions: Vec::new(),
        subtitles: Vec::new(),
    };
    let clip_meta = meta.get("clips").and_then(Value::as_array).cloned();

    let mut cursor = Tick::ZERO;
    let mut pending: Option<ImportedTransition> = None;
    let children = value
        .get("children")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for child in &children {
        let schema = child
            .get("OTIO_SCHEMA")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match schema {
            s if s.starts_with("Gap") => {
                cursor = cursor
                    + child
                        .get("source_range")
                        .map(range_duration)
                        .unwrap_or(Tick::ZERO)
                        .max(Tick::ZERO);
            }
            s if s.starts_with("Transition") => {
                let kind = transition_kind(child, warnings);
                let duration = child
                    .get("metadata")
                    .and_then(|meta| meta.get("ngy"))
                    .and_then(|meta| meta.get("duration"))
                    .and_then(Value::as_i64)
                    .map(Tick::from_micros)
                    .unwrap_or_else(|| {
                        let half = |key: &str| child.get(key).map(tick_from).unwrap_or(Tick::ZERO);
                        half("in_offset") + half("out_offset")
                    });
                pending = Some(ImportedTransition {
                    after: track.clips.len(),
                    kind,
                    duration,
                });
            }
            s if s.starts_with("Clip") => {
                let extras = clip_meta
                    .as_ref()
                    .and_then(|items| items.get(track.clips.len()))
                    .cloned()
                    .unwrap_or(Value::Null);
                match parse_clip(child, &extras, cursor, warnings) {
                    Some(clip) => {
                        // 片段在时间线上占的长度是「素材跨度 ÷ 倍率 + 定格」，
                        // 与工程里 `ClipItem::duration()` 同一条式子。
                        let motion = Tick::from_micros(
                            ((clip.source_out - clip.source_in).micros() as f64
                                / f64::from(clip.speed.clamp(0.01, 100.0)))
                            .round() as i64,
                        );
                        cursor = clip.start + motion + clip.hold.max(Tick::ZERO);
                        track.clips.push(clip);
                        if let Some(transition) = pending.take() {
                            track.transitions.push(transition);
                        }
                    }
                    None => pending = None,
                }
            }
            other => warnings.push(format!("轨道里有一条 {other} 不认识，已跳过")),
        }
    }

    if track.clips.is_empty() && track.subtitles.is_empty() {
        return None;
    }
    Some(track)
}

/// 一个片段。没有素材引用或时长非正时返回 `None`。
fn parse_clip(
    value: &Value,
    extras: &Value,
    start: Tick,
    warnings: &mut Vec<String>,
) -> Option<ImportedClip> {
    let url = value
        .get("media_reference")
        .and_then(|reference| reference.get("target_url"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let path = match path_from_url(url) {
        Some(path) => path,
        None => {
            let name = value.get("name").and_then(Value::as_str).unwrap_or("?");
            warnings.push(format!("片段「{name}」没有本地素材路径，已跳过"));
            return None;
        }
    };

    let range = value.get("source_range")?;
    let source_in = range_start(range);
    let span = range_duration(range);
    if span <= Tick::ZERO {
        let name = value.get("name").and_then(Value::as_str).unwrap_or("?");
        warnings.push(format!("片段「{name}」的时长为 0，已跳过"));
        return None;
    }

    let speed = extras
        .get("speed")
        .and_then(Value::as_f64)
        .map(|speed| speed as f32)
        .or_else(|| linear_warp_speed(value))
        .unwrap_or(1.0);

    let mut clip = ImportedClip {
        path,
        name: extras
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                value
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default(),
        start,
        source_in,
        source_out: source_in + span,
        speed,
        remap: extras
            .get("remap")
            .and_then(|remap| serde_json::from_value::<SpeedCurve>(remap.clone()).ok())
            .unwrap_or_default(),
        hold: extras
            .get("hold")
            .and_then(Value::as_i64)
            .map(Tick::from_micros)
            .unwrap_or(Tick::ZERO),
        effects: Vec::new(),
        keys: Keyframes::default(),
        color: ColorGrade::default(),
    };

    if let Some(Value::Array(items)) = extras.get("effects").cloned() {
        // 原始名字在反序列化时就丢了（认不出来的都落进 `EffectKind::Unknown`），
        // 所以先按字符串问一遍，提示里才能写出用户看到的那一行。
        for item in &items {
            if let Some(name) = item.get("kind").and_then(Value::as_str)
                && EffectKind::from(name.to_string()) == EffectKind::Unknown
            {
                warnings.push(format!("效果「{name}」不认识，已忽略"));
            }
        }
        match serde_json::from_value::<Vec<Effect>>(Value::Array(items)) {
            Ok(effects) => {
                clip.effects = effects
                    .into_iter()
                    .filter(|effect| effect.kind != EffectKind::Unknown)
                    .collect();
            }
            Err(error) => warnings.push(format!("效果栈读不出来，已忽略：{error}")),
        }
    }
    if let Some(keys) = extras.get("keys") {
        match serde_json::from_value::<Keyframes>(keys.clone()) {
            Ok(parsed) => clip.keys = parsed,
            Err(error) => warnings.push(format!("关键帧读不出来，已忽略：{error}")),
        }
    }
    if let Some(color) = extras.get("color") {
        match serde_json::from_value::<ColorGrade>(color.clone()) {
            Ok(parsed) => clip.color = parsed,
            Err(error) => warnings.push(format!("调色读不出来，已忽略：{error}")),
        }
    }
    Some(clip)
}

/// OTIO 的 `LinearTimeWarp`：`time_scalar` 就是倍率。
fn linear_warp_speed(value: &Value) -> Option<f32> {
    value
        .get("effects")
        .and_then(Value::as_array)?
        .iter()
        .find(|effect| {
            effect
                .get("OTIO_SCHEMA")
                .and_then(Value::as_str)
                .is_some_and(|schema| schema.starts_with("LinearTimeWarp"))
        })
        .and_then(|effect| effect.get("time_scalar"))
        .and_then(Value::as_f64)
        .map(|speed| speed as f32)
}

/// 转场类型：优先 `metadata.ngy.kind`，退回 OTIO 的 `transition_type`。
fn transition_kind(value: &Value, warnings: &mut Vec<String>) -> TransitionKind {
    if let Some(kind) = value
        .get("metadata")
        .and_then(|meta| meta.get("ngy"))
        .and_then(|meta| meta.get("kind"))
        .and_then(|kind| serde_json::from_value::<TransitionKind>(kind.clone()).ok())
    {
        return kind;
    }
    let name = value
        .get("transition_type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match name {
        "SMPTE_Dissolve" => TransitionKind::CrossDissolve,
        other => {
            if !other.is_empty() {
                warnings.push(format!("转场类型 {other} 没有对应，按交叉溶解导入"));
            }
            TransitionKind::CrossDissolve
        }
    }
}
