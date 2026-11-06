//! 片段级的编辑：选中、取消选中、删除、在播放头处切割。

use super::*;

impl PlayerApp {
    /// 在播放头处切开所有跨过它的片段。锁定的轨道由 `timeline::split_at` 跳过。
    pub(crate) fn split_at_playhead(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let at = project.ui.playhead;
        let outcome = crate::timeline::split_at(&project.sequence, at);

        match outcome {
            Ok(Some((sequence, cuts))) => {
                self.commit_edit(sequence, "在播放头处切割");
                self.flash(format!("已在播放头处切开 {cuts} 个片段"));
            }
            Ok(None) => self.flash("播放头位置没有可切的片段"),
            Err(error) => self.flash(format!("切割失败：{error}")),
        }
        self.poke();
        cx.notify();
    }

    /// 选中一个片段。目前是单选：时间线还没有需要多选才能做的事。
    pub(crate) fn select_clip(&mut self, clip: Id, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        project.ui.selected = vec![clip];
        cx.notify();
    }

    /// 点空白处取消选中。
    pub(crate) fn clear_selection(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        if project.ui.selected.is_empty() {
            return;
        }
        project.ui.selected.clear();
        cx.notify();
    }

    /// 删除选中的片段。
    ///
    /// `ripple` 为假时留空隙（默认），为真时同轨后面的内容往前补。校验（锁定轨道、
    /// 片段还在不在）都交给 `timeline::ops`。
    pub(crate) fn delete_selected_clips(&mut self, ripple: bool, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let clips = project.ui.selected.clone();
        let outcome = if ripple {
            if clips.is_empty() {
                Ok(None)
            } else {
                crate::timeline::apply(
                    &project.sequence,
                    &EditOp::RippleDelete {
                        clips: clips.clone(),
                    },
                )
                .map(Some)
            }
        } else {
            crate::timeline::remove_clips(&project.sequence, &clips)
        };

        match outcome {
            Ok(Some(sequence)) => {
                let message = if ripple {
                    format!("波纹删除 {} 个片段", clips.len())
                } else {
                    format!("已删除 {} 个片段", clips.len())
                };
                self.commit_edit(sequence, message.clone());
                if let Some(project) = self.project.as_mut() {
                    // 删掉的 id 不能留在选中集合里，否则下次操作会报「片段不存在」。
                    project.ui.selected.clear();
                }
                self.flash(message);
            }
            Ok(None) => self.flash("没有选中片段"),
            Err(error) => self.flash(format!("删除失败：{error}")),
        }
        self.poke();
        cx.notify();
    }
}
