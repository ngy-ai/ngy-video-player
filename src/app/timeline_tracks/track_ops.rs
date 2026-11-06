//! 轨道级的编辑：加轨、删轨、加 / 去掉转场。

use super::super::delete_track_confirm::{track_content_detail, track_has_content};
use super::*;

impl PlayerApp {
    /// 在当前片段与它左邻居之间加 / 去掉一个 0.5 秒的交叉溶解。
    ///
    /// 转场的界面入口就这一处：拖边角调溶接是下一版的事，先把「能产生转场」打通，
    /// 导出侧的 `xfade` 才有东西可测。只对紧挨着的同轨片段成立。
    pub(crate) fn toggle_transition(&mut self, clip: Id, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let Some(track) = project
            .sequence
            .tracks
            .iter()
            .find(|track| track.clip(clip).is_some())
        else {
            return;
        };
        let Some(index) = track.clip_index(clip) else {
            return;
        };
        if index == 0 {
            self.flash("左边没有相邻片段，加不了转场");
            return;
        }
        let previous = &track.clips[index - 1];
        if previous.end() != track.clips[index].start {
            self.flash("两个片段之间有空档，加不了转场");
            return;
        }
        let previous_id = previous.id;
        let existing = track
            .transitions
            .iter()
            .position(|transition| transition.before == previous_id && transition.after == clip);

        let mut sequence = project.sequence.clone();
        let mut message = "已去掉转场";
        match existing {
            Some(index) => {
                if let Some(track) = sequence.tracks.iter_mut().find(|t| t.clip(clip).is_some())
                    && index < track.transitions.len()
                {
                    track.transitions.remove(index);
                }
            }
            None => {
                let id = sequence.alloc_id();
                if let Some(track) = sequence.tracks.iter_mut().find(|t| t.clip(clip).is_some()) {
                    track.transitions.push(crate::timeline::Transition::new(
                        id,
                        previous_id,
                        clip,
                        crate::timeline::TransitionKind::CrossDissolve,
                        Tick::from_micros(500_000),
                    ));
                }
                message = "已加 0.5 秒交叉溶解";
            }
        }
        self.commit_edit(sequence, message);
        self.flash(message);
        self.poke();
        cx.notify();
    }

    /// 往序列末尾加一条轨道。
    ///
    /// 多轨导出（M3-T03/T04）的前提是序列里真的能有多条同类型轨道；模型一直支持，
    /// 但没有界面入口的话这套能力只是理论上的。新轨道默认开着、没锁。
    /// 加完把轨道区滚到最后一屏：轨道追加在末尾，不滚的话窗口矮一点就看不见。
    pub(crate) fn add_track(&mut self, kind: crate::timeline::TrackKind, cx: &mut Context<Self>) {
        let message = {
            let Some(project) = self.project.as_mut() else {
                return;
            };
            let id = project.sequence.add_track(kind);
            let name = project
                .sequence
                .track(id)
                .map(|track| track.name.clone())
                .unwrap_or_default();
            format!("已添加{}轨「{name}」", kind.label())
        };
        self.timeline.reveal_last_track();
        self.remember_history(message.clone());
        self.flash(message);
        self.poke();
        cx.notify();
    }

    /// 删掉一条轨道：轨道上有片段 / 转场时先确认，空轨道直接删。
    ///
    /// 锁定的轨道先给一句提示再返回，别让用户确认完才被告知「解锁了才能删」。
    pub(crate) fn delete_track(&mut self, track: Id, cx: &mut Context<Self>) {
        // 先把借用期间要用的值取出来，再调用 `&mut self` 的对话框 / 提示入口。
        let Some((locked, kind, name, detail, has_content)) = self
            .project
            .as_ref()
            .and_then(|project| project.sequence.track(track))
            .map(|target| {
                (
                    target.locked,
                    target.kind,
                    target.name.clone(),
                    track_content_detail(target),
                    track_has_content(target),
                )
            })
        else {
            return;
        };
        if locked {
            self.flash(format!("轨道「{name}」已锁定，先解锁再删除"));
            cx.notify();
            return;
        }
        if has_content {
            self.open_delete_track_confirm(track, kind, name, detail, cx);
            return;
        }
        self.delete_track_now(track, cx);
    }

    /// 真正执行删除。片段与转场跟着一起走；删完清空选中集合，
    /// 否则被删轨道上的片段 id 留着会让下一次操作报「片段不存在」。
    pub(crate) fn delete_track_now(&mut self, track: Id, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let Some(target) = project.sequence.track(track) else {
            return;
        };
        let message = format!("删除{}轨「{}」", target.kind.label(), target.name);
        let outcome = apply_edit(&project.sequence, &EditOp::RemoveTrack { track });

        match outcome {
            Ok(sequence) => {
                self.commit_edit(sequence, message.clone());
                if let Some(project) = self.project.as_mut() {
                    project.ui.selected.clear();
                }
                self.timeline_menu = None;
                self.flash(message);
            }
            Err(error) => self.flash(format!("删除轨道失败：{error}")),
        }
        self.poke();
        cx.notify();
    }
}
