//! 三点 / 四点编辑：素材库试听原片时设源入点 / 源出点，再按时间线播放头（或工作区）落位。
//!
//! 三个点：源入点、源出点、时间线入点。时间线的出点在有入点时是**第四点**（工作区
//! 区间），这时按最短者裁剪；只有入点或什么都没定时，出点只当上限用。
//!
//! 源入出点是**会话状态**，不进工程也不进撤销栈：它描述的是「我正要从这段素材里
//! 取哪一截」，换素材、退出试听就作废。时间线的 `UiState.in_point` / `out_point`
//! 是另一回事（工作区），两者互不覆盖——四点编辑只是**读**它。
//!
//! 目标轨道（轨道头上的标记）同样存在 `UiState` 里，随工程存盘。

use super::*;
use crate::timeline::{
    ClipSpec, EditOp, FourPointTrim, Id, MediaInfo, Placement, Tick, TrackKind,
    apply as apply_edit, four_point, resolve_target, three_point,
};

impl PlayerApp {
    /// 设源入点：取预览区当前播放位置。
    pub(crate) fn set_source_in(&mut self, cx: &mut Context<Self>) {
        if self.audition.is_none() {
            self.flash("先双击素材库里的素材试听原片，再设源入点");
            cx.notify();
            return;
        }
        let at = Tick::from_secs(self.snapshot.position.max(0.0));
        self.source_in = Some(at);
        // 出点落在新入点前面就没意义了，顺手清掉，避免留一个反向区间。
        if self.source_out.is_some_and(|out| out <= at) {
            self.source_out = None;
        }
        self.flash(format!("源入点 {}", format_time_precise(at.secs())));
        cx.notify();
    }

    /// 设源出点：取预览区当前播放位置。
    pub(crate) fn set_source_out(&mut self, cx: &mut Context<Self>) {
        if self.audition.is_none() {
            self.flash("先双击素材库里的素材试听原片，再设源出点");
            cx.notify();
            return;
        }
        let at = Tick::from_secs(self.snapshot.position.max(0.0));
        if self.source_in.is_some_and(|inside| at <= inside) {
            // 出点必须晚于入点：与其拒绝，不如把它当成新的入点，用户通常是想重设这一截。
            self.flash("出点要落在入点后面，先把播放往后挪一点");
            cx.notify();
            return;
        }
        self.source_out = Some(at);
        self.flash(format!("源出点 {}", format_time_precise(at.secs())));
        cx.notify();
    }

    /// 清掉整笔三点编辑：源素材与源入出点一起作废。换素材、退出试听、关面板时调用。
    pub(crate) fn clear_source(&mut self) {
        self.source_asset = None;
        self.source_in = None;
        self.source_out = None;
    }

    /// 试听条的文案：源区间与将放多长。没在三点编辑里返回 `None`。
    ///
    /// 只看 `source_asset`：拖一下播放头就结束试听，但源区间还在，条子要继续显示，
    /// 否则用户设好的入出点会「凭空消失」，再按 `,` 又提示去双击素材。
    pub(crate) fn source_range_label(&self) -> Option<String> {
        self.source_asset?;
        let source_in = self.source_in;
        let source_out = self.source_out;
        match (source_in, source_out) {
            (Some(inside), Some(out)) => Some(format!(
                "源 {} – {}（{}）",
                format_time_precise(inside.secs()),
                format_time_precise(out.secs()),
                format_time_precise((out - inside).secs())
            )),
            (Some(inside), None) => Some(format!(
                "源入点 {}，还没设出点（放到素材尾）",
                format_time_precise(inside.secs())
            )),
            (None, Some(out)) => Some(format!(
                "源出点 {}，从素材头取到这儿",
                format_time_precise(out.secs())
            )),
            (None, None) if self.audition.is_some() => Some(
                "`Alt I` / `Alt O` 设源入出点，`,` 插入、`.` 覆盖（工作区两端都定了走四点编辑）"
                    .to_string(),
            ),
            // 试听已经结束（播放头被挪走）：还留着素材是因为源区间可以接着用，
            // 这一行得说清楚怎么设点，而不是继续提示正在设点。
            (None, None) => {
                Some("还没定源区间：再双击素材试听，然后 `Alt I` / `Alt O` 设点".to_string())
            }
        }
    }

    /// 取消三点编辑：源素材与源入出点一起清掉，预览区不动。
    pub(crate) fn cancel_source(&mut self, cx: &mut Context<Self>) {
        if self.source_asset.is_none() {
            return;
        }
        self.clear_source();
        self.flash("已取消源区间");
        cx.notify();
    }

    /// 按三点 / 四点编辑把当前试听的素材放到时间线。
    ///
    /// - 工作区只有入点（或没有）：三点编辑，落点就是播放头，只放一路画面或声音。
    /// - 工作区入点 + 出点都定了：**四点编辑**，塞进这段槽位、按最短者裁剪；视频素材的
    ///   声音同时落到目标音频轨上（两段一起落一步撤销）。
    ///
    /// 目标轨来自轨道头上的目标标记（`UiState::target_*`），没指定就按「同类里第一条
    /// 没锁的」挑，规则在 [`resolve_target`]。
    pub(crate) fn place_from_source(&mut self, mode: Placement, cx: &mut Context<Self>) {
        // 认 `source_asset` 而不是 `audition`：定完源区间通常要先去时间线上点一下
        // 播放头（三点里的第三个点），那一下会结束试听。
        let Some(asset_id) = self.source_asset else {
            self.flash("先双击素材库里的素材定源区间，再按 `,` 插入、`.` 覆盖");
            cx.notify();
            return;
        };
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let Some(asset) = project.asset(asset_id) else {
            return;
        };
        let sequence = project.sequence();
        let media = asset.media.as_ref();
        let duration = media.map(|media| media.duration).unwrap_or(Tick::ZERO);
        let has_video = media.is_some_and(MediaInfo::has_video);
        let has_audio = media.is_some_and(MediaInfo::has_audio);
        let name = asset.display_name();
        let at = project.ui.playhead;
        let range = project.ui.range();
        let timeline_out = project.ui.out_point;
        let target_video = project.ui.target(TrackKind::Video);
        let target_audio = project.ui.target(TrackKind::Audio);

        let source_in = self.source_in.unwrap_or(Tick::ZERO);
        let source_out = self.source_out.unwrap_or(duration);

        let verb = match mode {
            Placement::Overwrite => "覆盖",
            Placement::Insert => "插入",
        };
        // 要落的每一段：轨道、素材起点、序列起点。
        let mut places: Vec<(Id, Tick, Tick)> = Vec::new();
        let message: String;
        let length;

        if let Some((tl_in, tl_out)) = range {
            let point = match four_point(source_in, source_out, tl_in, tl_out) {
                Ok(point) => point,
                Err(error) => {
                    self.flash(format!("放不上时间线：{error}"));
                    cx.notify();
                    return;
                }
            };
            length = point.duration();

            if has_video {
                let Some(track) = resolve_target(sequence, TrackKind::Video, target_video) else {
                    self.flash("没有可用的视频轨：先加一条，或者把锁解开");
                    cx.notify();
                    return;
                };
                places.push((track, point.source_in, point.at));
            }
            // 四点编辑是「一次成片」：视频素材的声音跟着落到音频轨上。没有音频轨时
            // 只放画面并说一声，而不是整笔失败——画面才是剪辑的主体。
            let mut audio_note = "";
            if has_audio {
                match resolve_target(sequence, TrackKind::Audio, target_audio) {
                    Some(track) => places.push((track, point.source_in, point.at)),
                    None if has_video => audio_note = "（没有可用的音频轨，只放了画面）",
                    None => {
                        self.flash("没有可用的音频轨：先加一条，或者把锁解开");
                        cx.notify();
                        return;
                    }
                }
            }

            let what = if places.len() > 1 {
                "画面与声音"
            } else {
                "片段"
            };
            let head = format!(
                "四点编辑：{verb} {what} {}{audio_note}",
                format_time_precise(length.secs())
            );
            message = match point.trim {
                Some(FourPointTrim::Source) => format!("{head}（源区间更长，按时间线区间截短）"),
                Some(FourPointTrim::Timeline) => format!("{head}（源区间只有这么长，后面留空）"),
                None => head,
            };
        } else {
            let point = match three_point(source_in, source_out, at, timeline_out) {
                Ok(point) => point,
                Err(error) => {
                    self.flash(format!("放不上时间线：{error}"));
                    cx.notify();
                    return;
                }
            };
            length = point.duration();
            let kind = if has_video {
                TrackKind::Video
            } else {
                TrackKind::Audio
            };
            let preferred = match kind {
                TrackKind::Video => target_video,
                _ => target_audio,
            };
            let Some(track) = resolve_target(sequence, kind, preferred) else {
                self.flash(format!(
                    "没有可用的{}轨：先加一条，或者把锁解开",
                    kind.label()
                ));
                cx.notify();
                return;
            };
            places.push((track, point.source_in, point.at));
            message = if point.clipped {
                format!(
                    "{verb} {} 秒（时间线只剩这么多，按出点截短了）",
                    format_time_precise(length.secs())
                )
            } else {
                format!("{verb} {} 秒", format_time_precise(length.secs()))
            };
        }

        // 两段一起落一步撤销：先后两次 commit 会让用户按一次 Ctrl+Z 只撤掉一半。
        let mut next = sequence.clone();
        let start = places.first().map(|(_, _, at)| *at).unwrap_or(at);
        for (track, source_in, at) in places {
            let spec =
                ClipSpec::new(asset_id, source_in, source_in + length).with_name(name.clone());
            let op = match mode {
                Placement::Overwrite => EditOp::Overwrite {
                    track,
                    at,
                    clip: spec,
                },
                Placement::Insert => EditOp::InsertAt {
                    track,
                    at,
                    clip: spec,
                },
            };
            match apply_edit(&next, &op) {
                Ok(sequence) => next = sequence,
                Err(error) => {
                    log_warn!("编辑放不进时间线：{error}");
                    self.error = Some(Banner::new(
                        "放不上时间线",
                        format!("{error}。\n\n检查轨道有没有被锁定，或者换个播放头位置。"),
                    ));
                    self.poke();
                    cx.notify();
                    return;
                }
            }
        }

        self.commit_edit(next, message.clone());
        // 播放头挪到这一段的末尾，接着按 `,` 就是往后接，而不是在同一处
        // 反复插入把前一段挤走。预览仍停在原片上（试听没结束），所以不刷新画面。
        if let Some(project) = self.project.as_mut() {
            project.ui.playhead = start + length;
        }
        self.follow_playhead();
        self.flash(message);
        self.poke();
        cx.notify();
    }
}
