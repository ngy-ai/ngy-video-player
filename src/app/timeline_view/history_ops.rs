//! 撤销 / 重做与历史列表：快照的压入、回退与选中清理。

use super::*;

impl PlayerApp {
    /// 把编辑结果写回工程并记入撤销历史。所有改序列的入口都走这里。
    pub(in crate::app) fn commit_edit(
        &mut self,
        sequence: crate::timeline::Sequence,
        label: impl Into<String>,
    ) {
        if let Some(project) = self.project.as_mut() {
            // 序列 id 属于工程：编辑纯函数若从零构造了新序列，也不能把它冲掉。
            let id = project.active_id();
            *project.sequence_mut() = sequence;
            project.sequence_mut().id = id;
        }
        self.remember_history(label);
    }

    /// 把当前序列快照进撤销栈。轨道开关这类不经 `ops` 的改动，改完手动调一次。
    ///
    /// `label` 会显示在历史列表里：快照之间长得几乎一样，没有说明就挑不出要回哪一步。
    pub(in crate::app) fn remember_history(&mut self, label: impl Into<String>) {
        let label = label.into();
        let Some(history) = self.history.as_mut() else {
            return;
        };
        if let Some(project) = self.project.as_ref() {
            history.push_labeled(project.sequence().clone(), label);
        }
        // 编辑路径最后都收敛到这里，自动保存的 30 秒计时也就挂在这一处。
        self.note_edit();
    }

    /// 撤销一步。
    pub(in crate::app) fn undo_edit(&mut self, cx: &mut Context<Self>) {
        let previous = self
            .history
            .as_mut()
            .and_then(|history| history.undo().cloned());
        let Some(sequence) = previous else {
            self.flash("没有可撤销的操作");
            return;
        };
        self.apply_history(sequence);
        self.flash("已撤销");
        cx.notify();
    }

    /// 重做一步。
    pub(in crate::app) fn redo_edit(&mut self, cx: &mut Context<Self>) {
        let next = self
            .history
            .as_mut()
            .and_then(|history| history.redo().cloned());
        let Some(sequence) = next else {
            self.flash("没有可重做的操作");
            return;
        };
        self.apply_history(sequence);
        self.flash("已重做");
        cx.notify();
    }

    /// 历史列表点了一行：直接跨到那一步，往前是回退、往后是前进。
    pub(in crate::app) fn jump_to_history_step(&mut self, index: usize, cx: &mut Context<Self>) {
        let target = self
            .history
            .as_mut()
            .and_then(|history| history.jump_to(index).cloned());
        let Some(sequence) = target else {
            return;
        };
        self.apply_history(sequence);
        self.flash(format!("已回到第 {} 步", index + 1));
        cx.notify();
    }

    /// 撤销 / 重做共用：换回历史状态，并把指向已消失片段的选中项清掉。
    fn apply_history(&mut self, sequence: crate::timeline::Sequence) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let id = project.active_id();
        *project.sequence_mut() = sequence;
        project.sequence_mut().id = id;
        // 先把存活的片段 id 收成集合：`ui` 与序列同属 project，不能一借一改。
        let alive: std::collections::HashSet<Id> = project
            .sequence()
            .clips()
            .map(|(_, clip)| clip.id)
            .collect();
        prune_selection(&mut project.ui.selected, &alive);
        // 撤销 / 重做同样是「内容改了」，要重新计时。
        self.note_edit();
        self.poke();
    }
}
