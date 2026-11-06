//! 从素材库往轨道上拖：落点换算、兼容性判定与松手后的片段生成。
//!
//! 和数据导入（`app::import`）的区别：那边是「把文件收进工程并排进时间线」，会自己挑轨道、
//! 视频和声音一起配对上；这边是「用户指了哪条轨道就放哪条」，所以只落在被拖到的那一条上。
//! 要连画面带声音一起排，用「导入到时间线」。

use super::super::timeline_tracks::snap_to_frame;
use super::super::*;
use crate::timeline::apply as apply_edit;
use crate::timeline::{
    AssetId, ClipSpec, EditError, EditOp, Id, Placement, Rational, Sequence, Tick, TrackKind,
};

/// 从素材库拖出来的素材。GPUI 按值类型分发，不会和时间线片段拖动、系统拖入混在一起。
#[derive(Clone)]
pub(crate) struct AssetDrag {
    /// 按素材在列表里的顺序排好，松手时按这个顺序首尾相接。
    pub(crate) items: Vec<AssetDragItem>,
    /// 拖动时跟着鼠标的文案：单个素材是名字，多个是「名字 等 N 个」。
    pub(crate) label: SharedString,
}

/// 拖拽里带的一份素材信息。时长与有无画面/声音在开始拖的时候就算好，
/// 落点预览每帧都要用，不该每帧去查工程。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AssetDragItem {
    pub(crate) id: AssetId,
    pub(crate) name: String,
    pub(crate) duration: Tick,
    pub(crate) has_video: bool,
    pub(crate) has_audio: bool,
}

/// 拖动素材时算出来的落点。画预览与松手后执行都用它。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AssetDrop {
    pub(crate) track: Id,
    pub(crate) start: Tick,
    pub(crate) duration: Tick,
    /// 这条轨道收不收得下这组素材。不成立时预览框画成拒绝色，松手也不改序列。
    pub(crate) ok: bool,
    /// 落点压到已有片段时怎么处理：默认覆盖，按住 `Shift` 是插入。
    pub(crate) mode: Placement,
}

/// 素材能不能放在这条轨道上。
///
/// 视频轨只接有画面的素材，音频轨只接有声音的；字幕轨要等 M9-T10。
/// 一个视频素材拖到音频轨是允许的——它确实有音轨，只是只取声音。
pub(crate) fn fits(kind: TrackKind, item: &AssetDragItem) -> bool {
    match kind {
        TrackKind::Video => item.has_video,
        TrackKind::Audio => item.has_audio,
        TrackKind::Subtitle => false,
    }
}

/// 这组素材都放得下才算可以放。混拖（音频 + 视频）落到任何一条轨道上都会被拒。
pub(crate) fn all_fit(kind: TrackKind, items: &[AssetDragItem]) -> bool {
    !items.is_empty() && items.iter().all(|item| fits(kind, item))
}

/// 预览框要画多长：素材首尾相接的总时长。
pub(crate) fn total_duration(items: &[AssetDragItem]) -> Tick {
    items
        .iter()
        .fold(Tick::ZERO, |sum, item| sum + item.duration)
}

/// 拒绝放下的原因。取第一条放不下的素材，说清楚它为什么不行。
pub(crate) fn reject_reason(kind: TrackKind, items: &[AssetDragItem]) -> String {
    let Some(item) = items.iter().find(|item| !fits(kind, item)) else {
        return String::new();
    };
    match kind {
        TrackKind::Video if !item.has_video => format!(
            "「{}」只有声音。视频轨放不了音频素材，把它拖到音频轨试试。",
            item.name
        ),
        TrackKind::Audio if !item.has_audio => {
            format!("「{}」没有音轨。音频轨只能放有声音的素材。", item.name)
        }
        TrackKind::Subtitle => "字幕轨还不接受拖拽，字幕条目要等字幕图层做完。".to_string(),
        _ => format!("「{}」放不进这条轨道。", item.name),
    }
}

/// 把一组素材首尾相接放到 `track` 的 `at` 处，按 `mode` 决定压到东西时怎么处理。
///
/// 在副本上排完再返回：中途有一段放不下（锁定、轨道类型不对）就整组放弃，
/// 不会留下「拖了三个只进去两个」的半截状态。
pub(crate) fn place_items(
    sequence: &Sequence,
    track: Id,
    at: Tick,
    items: &[AssetDragItem],
    mode: Placement,
) -> Result<Sequence, EditError> {
    let mut next = sequence.clone();
    let mut cursor = at;
    for item in items {
        let spec = ClipSpec::new(item.id, Tick::ZERO, item.duration).with_name(item.name.clone());
        let op = match mode {
            Placement::Overwrite => EditOp::Overwrite {
                track,
                at: cursor,
                clip: spec,
            },
            Placement::Insert => EditOp::InsertAt {
                track,
                at: cursor,
                clip: spec,
            },
        };
        next = apply_edit(&next, &op)?;
        cursor = cursor + item.duration;
    }
    Ok(next)
}

/// 拖动时跟着鼠标走的胶囊。
pub(crate) struct AssetGhost {
    pub(crate) name: SharedString,
}

impl Render for AssetGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(220.0))
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(4.0))
            .bg(gpui_kit::rgba(0x1d2a44f2))
            .border_1()
            .border_color(ui::accent())
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_size(px(11.0))
            .text_color(text())
            .child(self.name.clone())
    }
}

impl PlayerApp {
    /// 轨道上拖素材：算出落点。`lane_x` 是鼠标在轨道内容区里的横向像素。
    pub(crate) fn set_asset_drop(
        &mut self,
        track: Id,
        lane_x: f32,
        drag: &AssetDrag,
        shift: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let Some(target) = project.sequence.tracks.iter().find(|item| item.id == track) else {
            return;
        };
        let fps = project.sequence.fps;
        let duration = total_duration(&drag.items);
        let start = snap_start(self.timeline.time_at(lane_x), fps);
        // 先按帧对齐，再往片段边缘贴：差几帧在界面上看不出来，压到片段上就白吸附了。
        let tolerance = Tick::from_secs(f64::from(SNAP_EDGE_PX / self.timeline.zoom().max(1.0)));
        let start = snap_to_clip_edges(start, target, tolerance);
        let mode = if shift {
            Placement::Insert
        } else {
            Placement::Overwrite
        };
        // 覆盖与插入都能落在有片段的位置上，所以这里只看轨道收不收得下。
        let ok = !target.locked && all_fit(target.kind, &drag.items);
        let drop = AssetDrop {
            track,
            start,
            duration,
            ok,
            mode,
        };
        if self.asset_drop == Some(drop) {
            return;
        }
        self.asset_drop = Some(drop);
        cx.notify();
    }

    /// 松手：把素材变成轨道上的片段。落点没算出来（拖到了轨道外）就什么都不做。
    pub(crate) fn drop_assets(&mut self, track: Id, drag: &AssetDrag, cx: &mut Context<Self>) {
        let Some(drop) = self.asset_drop.take() else {
            cx.notify();
            return;
        };
        if drop.track != track || drag.items.is_empty() {
            cx.notify();
            return;
        }
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let kind = project
            .sequence
            .tracks
            .iter()
            .find(|item| item.id == track)
            .map(|item| item.kind)
            .unwrap_or(TrackKind::Video);

        if !all_fit(kind, &drag.items) {
            self.error = Some(Banner::new(
                "放不进这条轨道",
                reject_reason(kind, &drag.items),
            ));
            cx.notify();
            return;
        }

        let verb = match drop.mode {
            Placement::Overwrite => "覆盖",
            Placement::Insert => "插入",
        };
        match place_items(&project.sequence, track, drop.start, &drag.items, drop.mode) {
            Ok(sequence) => {
                let count = drag.items.len();
                let message = if count == 1 {
                    format!("{verb}素材")
                } else {
                    format!("{verb} {count} 个素材")
                };
                self.commit_edit(sequence, message.clone());
                self.flash(message);
            }
            Err(error) => {
                log_warn!("素材放不进时间线：{error}");
                self.error = Some(Banner::new(
                    "放不进这条轨道",
                    format!("{error}。\n\n检查这条轨道有没有被锁定，或者换个落点再试。"),
                ));
            }
        }
        self.poke();
        cx.notify();
    }
}

/// 轨道上算落点用的横向像素。抽出来是为了单测能脱离窗口验证换算。
pub(crate) fn lane_offset(position_x: f32, lane_left: f32) -> f32 {
    (position_x - lane_left).max(0.0)
}

/// 落点吸附到帧。与拖动片段用的是同一套，保证两条路径落下来位置一致。
pub(crate) fn snap_start(seconds: f64, fps: Rational) -> Tick {
    snap_to_frame(Tick::from_secs(seconds.max(0.0)), fps)
}

/// 落点离某个片段边缘不超过这么多像素时，直接贴上去。
const SNAP_EDGE_PX: f32 = 8.0;

/// 把落点吸到最近的片段左 / 右边缘。
///
/// 没有这一步，用户「放在空白处」其实总会压进前一段几帧——鼠标描不到那么准，
/// 结果每次都要看一句「和轨道上已有的片段重叠」。
pub(crate) fn snap_to_clip_edges(
    start: Tick,
    track: &crate::timeline::Track,
    tolerance: Tick,
) -> Tick {
    if tolerance <= Tick::ZERO {
        return start;
    }
    let mut best = start;
    let mut best_gap = tolerance;
    for clip in &track.clips {
        for edge in [clip.start, clip.end()] {
            let gap = if start >= edge {
                start - edge
            } else {
                edge - start
            };
            if gap <= best_gap {
                best_gap = gap;
                best = edge;
            }
        }
    }
    best
}

impl AssetDragItem {
    /// 从素材表里的一份素材构造。没有探测结果的素材时长为 0，放不进时间线，
    /// 所以调用方要先把它筛掉。
    pub(crate) fn from_asset(asset: &crate::timeline::Asset) -> Option<Self> {
        let media = asset.media.as_ref()?;
        if media.duration <= Tick::ZERO {
            return None;
        }
        Some(Self {
            id: asset.id,
            name: asset.display_name(),
            duration: media.duration,
            has_video: media.has_video(),
            has_audio: media.has_audio(),
        })
    }
}
