//! 撤销 / 重做：按操作粒度保存状态快照。
//!
//! 为什么存快照而不是逆操作：一个 `Split` 的逆操作要记住被切开的原片段，一个
//! `RippleDelete` 的逆操作要记住每条轨道被推了多少——这些信息写进逆操作就等于把
//! 快照拆散存放，出错时更难查。序列本身是纯数据，500 个片段的一份快照只有几十 KB，
//! 200 步的量级对这种工具完全可以接受。
//!
//! 撤销不跨会话：关掉工程就没了（见 `TODO.md` M1-T05 的「不做」）。
//!
//! 每一步还带一句界面说明：历史列表窗口不比较快照，只把说明列出来让用户挑一步跳过去。

use crate::timeline::{EditError, EditOp, Sequence, Tick, apply};

/// 最多能撤销多少步。栈满时丢最旧的一步。
pub const HISTORY_LIMIT: usize = 200;

/// 历史里的一步：界面上给用户看的说明 + 这一步**之后**的完整序列。
///
/// 说明和序列捆在一起而不是另开一个数组：栈满裁剪、撤销后开新分支丢重做，
/// 两处都要动数组，分开放迟早会错一位。
struct Step {
    label: String,
    sequence: Sequence,
}

/// 一条序列的编辑历史。
///
/// `steps` 栈底是打开工程（或新建工程）时的状态，`index` 指向当前状态；
/// 撤销就是往回挪一格。列表窗口按 `labels()` 把每一步画成一行。
pub struct UndoHistory {
    steps: Vec<Step>,
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
            steps: vec![Step {
                label: "初始状态".to_string(),
                sequence,
            }],
            index: 0,
            limit,
        }
    }

    pub fn current(&self) -> &Sequence {
        &self.steps[self.index].sequence
    }

    pub fn can_undo(&self) -> bool {
        self.index > 0
    }

    pub fn can_redo(&self) -> bool {
        self.index + 1 < self.steps.len()
    }

    /// 还能撤销几步。界面用来显示「还有 N 步」或直接判断。
    pub fn undo_steps(&self) -> usize {
        self.index
    }

    pub fn redo_steps(&self) -> usize {
        self.steps.len() - 1 - self.index
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    /// 当前停在第几步。列表窗口用它标出「当前」那一行。
    pub fn current_index(&self) -> usize {
        self.index
    }

    /// 按时间顺序给出每一步的说明，含初始状态与还没走到的重做分支。
    pub fn labels(&self) -> impl Iterator<Item = &str> + '_ {
        self.steps.iter().map(|step| step.label.as_str())
    }

    /// 记录一次操作之后的状态。返回是否真的入栈。
    ///
    /// 状态没变（例如把片段拖回原位）就不占一步；撤销之后再做新操作，
    /// 原来的重做分支被丢掉——这是所有编辑器的一致行为。
    pub fn push(&mut self, sequence: Sequence) -> bool {
        self.push_labeled(sequence, "编辑")
    }

    /// 带说明地记一步。界面知道刚才做了什么（导入、切割、删轨…），
    /// 列表窗口靠这句说明区分那 200 个长得一模一样的快照。
    pub fn push_labeled(&mut self, sequence: Sequence, label: impl Into<String>) -> bool {
        if *self.current() == sequence {
            return false;
        }
        self.steps.truncate(self.index + 1);
        self.steps.push(Step {
            label: label.into(),
            sequence,
        });
        self.index = self.steps.len() - 1;

        // 状态数最多 limit + 1（初始状态 + limit 步编辑）；超了就从栈底丢。
        if self.limit == 0 {
            let mut current = self.steps.pop().expect("至少有一个状态");
            current.label = "当前状态".to_string();
            self.steps.clear();
            self.steps.push(current);
            self.index = 0;
        } else if self.steps.len() > self.limit + 1 {
            let excess = self.steps.len() - (self.limit + 1);
            self.steps.drain(..excess);
            self.index = self.steps.len() - 1;
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

    /// 一步跨到历史里的任意位置：往前是撤销，往后是重做。
    /// 越界或本来就在这一步时返回 `None`，界面据此不提示「已撤销」。
    pub fn jump_to(&mut self, index: usize) -> Option<&Sequence> {
        if index >= self.steps.len() || index == self.index {
            return None;
        }
        self.index = index;
        Some(self.current())
    }

    /// 打开别的工程时重建历史（清空撤销与重做）。
    pub fn reset(&mut self, sequence: Sequence) {
        self.steps.clear();
        self.steps.push(Step {
            label: "初始状态".to_string(),
            sequence,
        });
        self.index = 0;
    }

    /// 当前状态下整条序列的时长。界面显示总时长时不必自己再算。
    pub fn duration(&self) -> Tick {
        self.current().duration()
    }
}

#[cfg(test)]
mod tests;
