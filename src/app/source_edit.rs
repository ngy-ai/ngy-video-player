//! 三点编辑：素材库试听原片时设源入点 / 源出点，再按时间线播放头落位。
//!
//! 三个点：源入点、源出点、时间线入点。时间线的出点（工作区终点）只当上限用——
//! 空间不够就截断并提示，不会把片段放到工作区外面。
//!
//! 源入出点是**会话状态**，不进工程也不进撤销栈：它描述的是「我正要从这段素材里
//! 取哪一截」，换素材、退出试听就作废。时间线的 `UiState.in_point` / `out_point`
//! 是另一回事（工作区），两者互不覆盖。

use super::*;
use crate::timeline::{
    ClipSpec, EditOp, MediaInfo, Placement, ThreePointError, Tick, TrackKind, apply as apply_edit,
    three_point,
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
            (None, None) if self.audition.is_some() => {
                Some("`Alt I` / `Alt O` 设源入出点，`,` 插入、`.` 覆盖".to_string())
            }
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

    /// 按三点编辑把当前试听的素材放到时间线。
    ///
    /// 目标轨道按素材能力挑：有画面进第一条未锁定的视频轨，纯音频进第一条未锁定的音频轨。
    /// 三点编辑一次只放一路，视频素材的声音不跟着走（那要用「导入到时间线」）。
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
        let duration = asset
            .media
            .as_ref()
            .map(|media| media.duration)
            .unwrap_or(Tick::ZERO);
        let at = project.ui.playhead;
        let timeline_out = project.ui.out_point;

        let source_in = self.source_in.unwrap_or(Tick::ZERO);
        let source_out = self.source_out.unwrap_or(duration);
        let point = match three_point(source_in, source_out, at, timeline_out) {
            Ok(point) => point,
            Err(error) => {
                self.report_three_point(error);
                cx.notify();
                return;
            }
        };

        let kind = if asset.media.as_ref().is_some_and(MediaInfo::has_video) {
            TrackKind::Video
        } else {
            TrackKind::Audio
        };
        let Some(track) = project
            .sequence
            .tracks
            .iter()
            .find(|track| track.kind == kind && !track.locked)
            .map(|track| track.id)
        else {
            self.flash(format!(
                "没有可用的{}轨：先加一条，或者把锁解开",
                kind.label()
            ));
            cx.notify();
            return;
        };

        let name = asset.display_name();
        let spec = ClipSpec::new(asset_id, point.source_in, point.source_out).with_name(name);
        let op = match mode {
            Placement::Overwrite => EditOp::Overwrite {
                track,
                at: point.at,
                clip: spec,
            },
            Placement::Insert => EditOp::InsertAt {
                track,
                at: point.at,
                clip: spec,
            },
        };

        match apply_edit(&project.sequence, &op) {
            Ok(sequence) => {
                let verb = match mode {
                    Placement::Overwrite => "覆盖",
                    Placement::Insert => "插入",
                };
                let message = if point.clipped {
                    format!(
                        "{verb} {} 秒（时间线只剩这么多，按出点截短了）",
                        format_time_precise(point.duration().secs())
                    )
                } else {
                    format!("{verb} {} 秒", format_time_precise(point.duration().secs()))
                };
                self.commit_edit(sequence, message.clone());
                // 播放头挪到这一段的末尾，接着按 `,` 就是往后接，而不是在同一处
                // 反复插入把前一段挤走。预览仍停在原片上（试听没结束），所以不刷新画面。
                if let Some(project) = self.project.as_mut() {
                    project.ui.playhead = point.at + point.duration();
                }
                self.follow_playhead();
                self.flash(message);
            }
            Err(error) => {
                log_warn!("三点编辑放不进时间线：{error}");
                self.error = Some(Banner::new(
                    "放不上时间线",
                    format!("{error}。\n\n检查轨道有没有被锁定，或者换个播放头位置。"),
                ));
            }
        }
        self.poke();
        cx.notify();
    }

    fn report_three_point(&mut self, error: ThreePointError) {
        self.flash(format!("放不上时间线：{error}"));
    }
}
