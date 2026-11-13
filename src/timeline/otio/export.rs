//! 序列 → OTIO JSON。
//!
//! 结构上是标准 `Timeline`：`Stack` 下面每条视频 / 音频轨一个 `Track`，轨道里
//! 按时间顺序摆 `Gap` / `Clip` / `Transition`。字幕轨不进结构（OTIO 没有字幕轨
//! 这个概念，硬塞一条空 `Video` 轨只会在对方软件里留一堆空轨），连同效果栈、
//! 关键帧、调色、片尾定格一起放进 `metadata.ngy`。

use super::*;
use crate::timeline::{Project, Sequence, TrackKind};

/// 把一条序列导出成 `.otio` 文本（2 空格缩进的 JSON）。
///
/// 素材路径按工程里的绝对路径写 `file://` URL：对方软件打开时素材就在手边。
/// 找不到绝对路径（工程还没保存、素材没解析过）时退回工程里存的相对路径，
/// 相对路径在对方那里通常连不上，但它至少说清了「原本是哪个文件」。
pub fn export_sequence(project: &Project, sequence: &Sequence) -> String {
    let fps = sequence.fps;
    let mut tracks = Vec::new();
    let mut subtitle_tracks = Vec::new();

    for track in &sequence.tracks {
        match track.kind {
            TrackKind::Subtitle => {
                subtitle_tracks.push(json!({
                    "name": track.name,
                    "muted": track.muted,
                    "locked": track.locked,
                    "items": track.subtitles,
                }));
                continue;
            }
            TrackKind::Video | TrackKind::Audio => {}
        }

        let mut children = Vec::new();
        let mut cursor = Tick::ZERO;
        let mut clip_meta = Vec::new();
        for clip in &track.clips {
            if clip.start > cursor {
                children.push(json!({
                    "OTIO_SCHEMA": "Gap.1",
                    "source_range": time_range(Tick::ZERO, clip.start - cursor, fps),
                }));
            }
            if let Some(transition) = track.transitions.iter().find(|item| item.after == clip.id) {
                let half = Tick::from_micros(transition.duration.micros() / 2);
                children.push(json!({
                    "OTIO_SCHEMA": "Transition.1",
                    "name": transition.kind.label(),
                    "transition_type": otio_transition_type(transition.kind),
                    "in_offset": rational_time(half, fps),
                    "out_offset": rational_time(half, fps),
                    "metadata": {"ngy": {
                        "kind": transition.kind,
                        "duration": transition.duration.micros(),
                    }},
                }));
            }

            let url = project
                .asset_path(clip.asset_id)
                .map(file_url)
                .unwrap_or_default();
            // 速度曲线在 OTIO 里只有一个平均倍率能表达（`LinearTimeWarp` 是常量）：
            // 取 `素材跨度 ÷ 实际时长`，对方的时长与后面片段的位置才对得上。
            // 曲线本身另存一份在 `metadata.ngy` 里，本程序重新导入时能原样接回来。
            let speed = if clip.has_remap() {
                (clip.span().secs() / clip.motion().secs().max(1e-6)) as f32
            } else {
                clip.rate()
            };
            let mut item = json!({
                "OTIO_SCHEMA": "Clip.2",
                "name": clip_label(project, clip),
                "source_range": time_range(clip.source_in, clip.span(), fps),
                "media_reference": {
                    "OTIO_SCHEMA": "ExternalReference.1",
                    "target_url": url,
                    "available_range": time_range(Tick::ZERO, clip.span(), fps),
                },
            });
            // 变速按 OTIO 的写法挂一个 LinearTimeWarp，对方软件才算出正确时长。
            if (speed - 1.0).abs() > 1e-4 {
                item["effects"] = json!([{
                    "OTIO_SCHEMA": "LinearTimeWarp.1",
                    "name": "speed",
                    "effect_name": "speed",
                    "time_scalar": speed,
                }]);
            }
            children.push(item);
            clip_meta.push(json!({
                "speed": speed,
                "hold": clip.hold.micros(),
                "name": clip.name,
                "effects": clip.effects,
                "keys": clip.keys,
                "color": clip.color,
                "remap": clip.remap,
            }));
            cursor = clip.end();
        }

        tracks.push(json!({
            "OTIO_SCHEMA": "Track.1",
            "name": track.name,
            "kind": if track.kind == TrackKind::Video { "Video" } else { "Audio" },
            "metadata": {"ngy": {
                "muted": track.muted,
                "locked": track.locked,
                "gain": track.gain,
                "solo": track.solo,
                "clips": clip_meta,
            }},
            "children": children,
        }));
    }

    let markers: Vec<Value> = sequence
        .markers
        .iter()
        .map(|marker| json!({"at": marker.at.micros(), "name": marker.name}))
        .collect();

    let timeline = json!({
        "OTIO_SCHEMA": "Timeline.1",
        "name": sequence.name,
        "global_start_time": rational_time(Tick::ZERO, fps),
        "metadata": {"ngy": {
            "format": META_VERSION,
            "fps": format!("{}/{}", fps.num(), fps.den()),
            "width": sequence.width,
            "height": sequence.height,
            "markers": markers,
            "subtitle_tracks": subtitle_tracks,
        }},
        "tracks": {
            "OTIO_SCHEMA": "Stack.1",
            "name": "tracks",
            "children": tracks,
        },
    });

    serde_json::to_string_pretty(&timeline).unwrap_or_else(|_| "{}".to_string())
}

/// 片段在对方软件里显示的名字：片段名 → 素材名 → 兜底。
fn clip_label(project: &Project, clip: &crate::timeline::ClipItem) -> String {
    if !clip.name.is_empty() {
        return clip.name.clone();
    }
    project
        .asset(clip.asset_id)
        .map(|asset| asset.display_name())
        .unwrap_or_else(|| "素材".to_string())
}

/// OTIO 的转场类型名。只有交叉溶解是标准类型，其余三种在 OTIO 里没有对应，
/// 写 `Custom_Transition` 并靠 `metadata.ngy.kind` 还原。
fn otio_transition_type(kind: crate::timeline::TransitionKind) -> &'static str {
    match kind {
        crate::timeline::TransitionKind::CrossDissolve => "SMPTE_Dissolve",
        _ => "Custom_Transition",
    }
}
