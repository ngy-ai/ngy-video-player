//! 拖动落点：算出落点、松手时把落点交给 `timeline::ops`。
//!
//! 修剪的修饰键语义也在这里（拖动中读到就存下来，松手时用）：
//! `Alt` 波纹修剪 / `Shift` 拖右端滚动编辑 / `Alt` 拖中部滑移 / `Ctrl` 拖中部滑动。

use super::*;

impl PlayerApp {
    /// 记下落点。相同落点直接返回，拖动时指针每移动一个像素都重绘整窗太浪费。
    pub(super) fn set_timeline_drop(&mut self, drop: TimelineDrop, cx: &mut Context<Self>) {
        if self.timeline_drop == Some(drop) {
            return;
        }
        self.timeline_drop = Some(drop);
        cx.notify();
    }

    /// 记下关键帧标记的落点。落点没变就不重绘：拖动时每移动一个像素都重绘整窗太浪费。
    ///
    /// 标记只在片段自己的轨道上落：拖到别的轨道既不挪片段，也没有明确语义。
    pub(super) fn set_keyframe_drop(
        &mut self,
        track: Id,
        clip: Id,
        from: Tick,
        lane_x: f32,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        if project.sequence().clip_track(clip) != Some(track) {
            return;
        }
        let fps = project.sequence().fps;
        let Some(item) = project.sequence().clip(clip) else {
            return;
        };
        let seconds = self.timeline.time_at(lane_x).max(0.0);
        let to = (snap_to_frame(Tick::from_secs(seconds), fps) - item.start)
            .max(Tick::ZERO)
            .min(item.duration());
        let drop = KeyframeDrop { clip, from, to };
        if self.keyframe_drop == Some(drop) {
            return;
        }
        self.keyframe_drop = Some(drop);
        cx.notify();
    }

    /// 松手：把「哪个时刻的点挪到哪儿」交给 `timeline::ops`，锁轨与夹取都在那边。
    pub(super) fn drop_keyframe(&mut self, drag: &KeyframeDrag, cx: &mut Context<Self>) {
        let Some(drop) = self.keyframe_drop.take() else {
            cx.notify();
            return;
        };
        // 拖了一下又放回原处不该占一步历史。
        if drop.clip != drag.clip || drop.from != drag.from || drop.to == drop.from {
            cx.notify();
            return;
        }
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let op = EditOp::MoveKeyframe {
            clip: drop.clip,
            from: drop.from,
            to: drop.to,
        };
        let outcome = apply_edit(project.sequence(), &op);
        match outcome {
            Ok(sequence) => {
                self.commit_edit(sequence, "移动关键帧");
                self.flash(format!("关键帧挪到 {:.2} 秒（片段内）", drop.to.secs()));
            }
            Err(error) => self.flash(format!("移动关键帧失败：{error}")),
        }
        self.poke();
        cx.notify();
    }

    /// 切换修剪工具。只影响之后的拖动语义，不进撤销栈（工具不是工程内容）。
    pub(in crate::app) fn set_trim_tool(&mut self, tool: TrimTool, cx: &mut Context<Self>) {
        if self.project.is_none() {
            return;
        }
        if self.trim_tool != tool {
            self.trim_tool = tool;
            self.flash(format!(
                "修剪工具：{}（拖片段两端用；点标题条上的工具名也能切）",
                tool.label()
            ));
        }
        cx.notify();
    }

    /// 松手：把修剪落点交给 `timeline::ops`。
    ///
    /// 语义由「当前工具 + 修饰键」决定，规则在 [`effective_trim`]：`Alt` 临时切成波纹，
    /// `Shift` 拖右端且下一段紧挨着时是滚动编辑，其余按工具走（滚动拖左端 / 滑移 / 滑动
    /// 这三种在端点上不成立，退回普通修剪）。拖回原处不占一步历史。
    pub(super) fn drop_trim(&mut self, drag: &TrimDrag, cx: &mut Context<Self>) {
        let Some(drop) = self.trim_drop.take() else {
            cx.notify();
            return;
        };
        if drop.clip != drag.clip || drop.side != drag.side {
            cx.notify();
            return;
        }
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let sequence = project.sequence();
        let Some(clip) = sequence.clip(drop.clip) else {
            return;
        };
        let (start, end) = (clip.start, clip.end());
        let old_length = end - start;
        // 后面有没有紧挨着的片段：滚动编辑的前提。
        let adjacent_next = sequence
            .clip_track(drop.clip)
            .and_then(|track| sequence.track(track))
            .and_then(|track| {
                let index = track.clip_index(drop.clip)?;
                Some(
                    track
                        .clips
                        .get(index + 1)
                        .is_some_and(|next| next.start == end),
                )
            })
            .unwrap_or(false);
        let chosen = effective_trim(
            self.trim_tool,
            drop.side,
            adjacent_next,
            drop.alt,
            drop.shift,
        );
        // 没动过（落点还是原来那一端）就别落历史。
        let unmoved = match drop.side {
            TrimSide::Start => drop.at == start,
            TrimSide::End => drop.at == end,
        };
        if unmoved {
            cx.notify();
            return;
        }

        let roll = chosen == TrimTool::Roll;
        let op = match chosen {
            TrimTool::Roll => EditOp::Roll {
                clip: drop.clip,
                by: drop.at - end,
            },
            TrimTool::Ripple => match drop.side {
                TrimSide::Start => EditOp::RippleTrimStart {
                    clip: drop.clip,
                    to: drop.at,
                },
                TrimSide::End => EditOp::RippleTrimEnd {
                    clip: drop.clip,
                    to: drop.at,
                },
            },
            _ => match drop.side {
                TrimSide::Start => EditOp::TrimStart {
                    clip: drop.clip,
                    to: drop.at,
                },
                TrimSide::End => EditOp::TrimEnd {
                    clip: drop.clip,
                    to: drop.at,
                },
            },
        };
        let tool = match chosen {
            TrimTool::Roll => "滚动编辑",
            TrimTool::Ripple => "波纹修剪",
            _ => "修剪",
        };
        let outcome = apply_edit(sequence, &op);
        match outcome {
            Ok(next) => {
                // 新的长度按「另一端不动」算：拖的哪一端，另一端就是基准。
                let length = match drop.side {
                    TrimSide::Start => end - drop.at,
                    TrimSide::End => drop.at - start,
                }
                .max(Tick::ZERO);
                let message = if roll {
                    format!("滚动编辑：接缝挪到 {:.2} 秒", drop.at.secs())
                } else {
                    format!("{tool}：{:.2} → {:.2} 秒", old_length.secs(), length.secs())
                };
                self.commit_edit(next, tool);
                self.flash(message);
                // 松手之后弹双画面窗口：切点前后两帧 + 两帧时间码。
                self.open_trim_window(drop.clip, drop.side, cx);
            }
            Err(error) => self.flash(format!("{tool}失败：{error}")),
        }
        self.poke();
        cx.notify();
    }

    /// 松手：把落点交给 `timeline::ops`，重叠 / 锁定 / 轨道类型这些校验都在那边。
    ///
    /// 语义由「当前工具 + 修饰键」决定（拖动时读键、松手时用）：
    /// - 工具是滑移（`Y`）或按着 `Alt`：滑移，片段位置不动、素材内容左右移。
    /// - 工具是滑动（`U`）或按着 `Ctrl`（`⌘`）：滑动，片段左右移动、两侧邻居伸缩。
    /// - 其余：移动片段；`Shift` 还是「插入」落位。
    ///
    /// 滑移 / 滑动只在同一轨道内成立；拖到别的轨上就按普通移动处理，免得「滑到一半
    /// 换了轨」把素材对错地方。
    pub(super) fn drop_timeline_clip(&mut self, drag: &TimelineDrag, cx: &mut Context<Self>) {
        let clip = drag.clip;
        let (alt, _shift, ctrl) = drag.mods.get();
        let slip = alt || self.trim_tool == TrimTool::Slip;
        let slide = !alt && (ctrl || self.trim_tool == TrimTool::Slide);
        let Some(drop) = self.timeline_drop.take() else {
            return;
        };
        if drop.clip != clip || self.project.is_none() {
            cx.notify();
            return;
        }

        // 滑移 / 滑动：同一轨道内的相对位移，落点用 `start` 与原位置的差值算。
        let same_track = self
            .project
            .as_ref()
            .and_then(|project| project.sequence().clip_track(clip))
            .is_some_and(|track| track == drop.to_track);
        let current_start = self
            .project
            .as_ref()
            .and_then(|project| project.sequence().clip(clip))
            .map(|item| item.start);

        let op = if same_track && (slip || slide) {
            let Some(from) = current_start else {
                return;
            };
            let by = drop.start - from;
            let (op, tool) = if slip {
                (EditOp::Slip { clip, by }, "滑移")
            } else {
                (EditOp::Slide { clip, by }, "滑动")
            };
            match self.project.as_ref() {
                Some(project) => match apply_edit(project.sequence(), &op) {
                    Ok(sequence) => {
                        self.commit_edit(sequence, tool);
                        self.flash(format!("{tool} {:.2} 秒", by.secs()));
                        self.poke();
                        cx.notify();
                        return;
                    }
                    Err(error) => {
                        self.flash(format!("{tool}失败：{error}"));
                        self.poke();
                        cx.notify();
                        return;
                    }
                },
                None => return,
            }
        } else {
            // 默认覆盖：拖到别的片段上就把那一段盖掉，而不是弹一句「重叠」拒收。
            match drop.mode {
                Placement::Overwrite => EditOp::MoveOverwrite {
                    clip,
                    to_track: drop.to_track,
                    start: drop.start,
                },
                Placement::Insert => EditOp::MoveInsert {
                    clip,
                    to_track: drop.to_track,
                    start: drop.start,
                },
            }
        };
        let outcome = match self.project.as_ref() {
            Some(project) => apply_edit(project.sequence(), &op),
            None => return,
        };
        match outcome {
            Ok(sequence) => {
                self.commit_edit(sequence, "移动片段");
                self.flash("已移动片段");
            }
            Err(error) => self.flash(format!("移动失败：{error}")),
        }
        self.poke();
        cx.notify();
    }
}
