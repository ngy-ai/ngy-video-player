//! 撤销 / 重做：按操作粒度保存状态快照。
//!
//! 为什么存快照而不是逆操作：一个 `Split` 的逆操作要记住被切开的原片段，一个
//! `RippleDelete` 的逆操作要记住每条轨道被推了多少——这些信息写进逆操作就等于把
//! 快照拆散存放，出错时更难查。序列本身是纯数据，500 个片段的一份快照只有几十 KB，
//! 200 步的量级对这种工具完全可以接受。
//!
//! 撤销不跨会话：关掉工程就没了（见 `TODO.md` M1-T05 的「不做」）。

use crate::timeline::{EditError, EditOp, Sequence, Tick, apply};

/// 最多能撤销多少步。栈满时丢最旧的一步。
pub const HISTORY_LIMIT: usize = 200;

/// 一条序列的编辑历史。
///
/// `states` 的每个元素都是一次操作**之后**的完整序列，栈底是打开工程（或新建工程）
/// 时的状态。`index` 指向当前状态；撤销就是往回挪一格。
pub struct UndoHistory {
    states: Vec<Sequence>,
    index: usize,
    limit: usize,
}

impl UndoHistory {
    pub fn new(sequence: Sequence) -> Self {
        Self::with_limit(sequence, HISTORY_LIMIT)
    }

    /// 自定义上限。上限指「最多能撤销几步」，0 表示只保留当前状态、不能撤销。
    pub fn with_limit(sequence: Sequence, limit: usize) -> Self {
        Self {
            states: vec![sequence],
            index: 0,
            limit,
        }
    }

    pub fn current(&self) -> &Sequence {
        &self.states[self.index]
    }

    pub fn can_undo(&self) -> bool {
        self.index > 0
    }

    pub fn can_redo(&self) -> bool {
        self.index + 1 < self.states.len()
    }

    /// 还能撤销几步。界面用来显示「还有 N 步」或直接判断。
    pub fn undo_steps(&self) -> usize {
        self.index
    }

    pub fn redo_steps(&self) -> usize {
        self.states.len() - 1 - self.index
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    /// 记录一次操作之后的状态。返回是否真的入栈。
    ///
    /// 状态没变（例如把片段拖回原位）就不占一步；撤销之后再做新操作，
    /// 原来的重做分支被丢掉——这是所有编辑器的一致行为。
    pub fn push(&mut self, sequence: Sequence) -> bool {
        if *self.current() == sequence {
            return false;
        }
        self.states.truncate(self.index + 1);
        self.states.push(sequence);
        self.index = self.states.len() - 1;

        // 状态数最多 limit + 1（初始状态 + limit 步编辑）；超了就从栈底丢。
        if self.limit == 0 {
            let current = self.states.pop().expect("至少有一个状态");
            self.states.clear();
            self.states.push(current);
            self.index = 0;
        } else if self.states.len() > self.limit + 1 {
            let excess = self.states.len() - (self.limit + 1);
            self.states.drain(..excess);
            self.index = self.states.len() - 1;
        }
        true
    }

    /// 应用一次编辑并入栈。失败时历史不变，错误原样返回给界面。
    pub fn edit(&mut self, op: &EditOp) -> Result<&Sequence, EditError> {
        let next = apply(self.current(), op)?;
        self.push(next);
        Ok(self.current())
    }

    /// 撤销一步，返回撤销后的状态；没有可撤销的返回 `None`。
    pub fn undo(&mut self) -> Option<&Sequence> {
        if !self.can_undo() {
            return None;
        }
        self.index -= 1;
        Some(self.current())
    }

    /// 重做一步，返回重做后的状态。
    pub fn redo(&mut self) -> Option<&Sequence> {
        if !self.can_redo() {
            return None;
        }
        self.index += 1;
        Some(self.current())
    }

    /// 打开别的工程时重建历史（清空撤销与重做）。
    pub fn reset(&mut self, sequence: Sequence) {
        self.states.clear();
        self.states.push(sequence);
        self.index = 0;
    }

    /// 当前状态下整条序列的时长。界面显示总时长时不必自己再算。
    pub fn duration(&self) -> Tick {
        self.current().duration()
    }
}

#[cfg(test)]
mod tests;
