//! 轨道级的编辑：加轨、删轨、加 / 去掉转场。

use super::super::delete_track_confirm::{track_content_detail, track_has_content};
use super::*;

impl PlayerApp {
    /// 设置（或改）接缝上的转场。没有就新建，有就改类型与时长。
    ///
    /// 时长会被钳到两侧余量以内：用户可以随便点，但落进工程里的值必须是导出
    /// 能兑现的那个（余量不够时导出只会退化成硬切，见 `timeline::export`）。
    pub(crate) fn apply_transition(
        &mut self,
        after: Id,
        kind: crate::timeline::TransitionKind,
        duration: Tick,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        match crate::timeline::set_transition(project.sequence(), after, kind, duration) {
            Ok(applied) => {
                let label = crate::timeline::transition_at(&applied.sequence, after)
                    .map(|transition| transition.kind.label())
                    .unwrap_or_else(|| kind.label());
                let message = if applied.clamped {
                    format!(
                        "两侧余量不够，{label}按 {:.2} 秒生效",
                        applied.duration.secs()
                    )
                } else {
                    format!("转场：{label} {:.2} 秒", applied.duration.secs())
                };
                self.commit_edit(applied.sequence, message.clone());
                self.flash(message);
            }
            Err(error) => self.flash(error.to_string()),
        }
        self.poke();
        cx.notify();
    }

    /// 去掉接缝上的转场。本来就没有时给一句提示，不留空操作。
    pub(crate) fn drop_transition(&mut self, after: Id, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        match crate::timeline::remove_transition(project.sequence(), after) {
            Ok(sequence) => {
                self.commit_edit(sequence, "去掉转场");
                self.flash("已去掉转场");
            }
            Err(error) => self.flash(error.to_string()),
        }
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
            let id = project.sequence_mut().add_track(kind);
            let name = project
                .sequence()
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
            .and_then(|project| project.sequence().track(track))
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
        let Some(target) = project.sequence().track(track) else {
            return;
        };
        let message = format!("删除{}轨「{}」", target.kind.label(), target.name);
        let outcome = apply_edit(project.sequence(), &EditOp::RemoveTrack { track });

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
