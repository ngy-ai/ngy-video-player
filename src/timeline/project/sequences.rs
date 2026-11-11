//! 工程里的多条序列：切换、新建、改名、删除与读盘后的自愈。
//!
//! 序列 id 由工程级计数器分配，和每条序列内部的 `next_id` 是两个空间：不同序列里
//! 可以有同号的轨道 / 片段，只有 `(sequence_id, object_id)` 才唯一。界面状态与撤销栈
//! 仍是工程级一份，切换时只复位「正在看的位置」，取舍见 `docs/adr/0009`。

use super::Project;
use crate::timeline::{Id, Rational, Sequence, SequenceSpec, Tick};
use std::collections::HashSet;
use std::fmt;

/// 序列管理操作失败的原因。界面直接把它写进提示，所以要给可执行的下一步。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SequenceError {
    NotFound(Id),
    /// 最后一条序列不能删：工程至少要有一条时间线。
    IsLast,
    EmptyName,
}

impl fmt::Display for SequenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SequenceError::NotFound(id) => write!(f, "找不到序列 {}，请刷新列表再试", id),
            SequenceError::IsLast => write!(f, "至少要留一条序列，先新建一条再删这条"),
            SequenceError::EmptyName => write!(f, "序列名不能为空，填个名字再确定"),
        }
    }
}

impl std::error::Error for SequenceError {}

impl Project {
    /// 当前序列在列表里的下标。指针失效时退回第一条，不让工程变成「没有当前序列」。
    fn active_index(&self) -> usize {
        self.sequences
            .iter()
            .position(|sequence| sequence.id == self.active_sequence)
            .unwrap_or(0)
    }

    /// 当前正在编辑的序列。工程保证列表非空（读盘时自愈，见 `repair_sequences`）。
    pub fn sequence(&self) -> &Sequence {
        &self.sequences[self.active_index()]
    }

    pub fn sequence_mut(&mut self) -> &mut Sequence {
        let index = self.active_index();
        &mut self.sequences[index]
    }

    /// 全部序列，按创建顺序。列表本身不对外可变：增删改都走下面的方法。
    pub fn sequences(&self) -> &[Sequence] {
        &self.sequences
    }

    /// 当前序列的 id。用实际指向的那条，不用可能失效的指针字段。
    pub fn active_id(&self) -> Id {
        self.sequence().id
    }

    pub fn sequence_by_id(&self, id: Id) -> Option<&Sequence> {
        self.sequences.iter().find(|sequence| sequence.id == id)
    }

    /// 给新序列分配 id。手改过的计数器也在这里推一把，避免和已有序列撞号。
    pub fn alloc_sequence_id(&mut self) -> Id {
        let max = self
            .sequences
            .iter()
            .map(|sequence| sequence.id.raw())
            .max()
            .unwrap_or(0);
        let raw = self.next_sequence_id.max(max.saturating_add(1)).max(1);
        self.next_sequence_id = raw.saturating_add(1);
        Id::new(raw)
    }

    /// 新建一条空序列。名字空着时给「序列 N」，规格由调用方给（界面沿用当前序列）。
    /// 返回 id 但不自动切过去：调用方还要停播、复位预览，那些是界面层的事。
    pub fn add_sequence(&mut self, name: &str, spec: SequenceSpec) -> Id {
        let id = self.alloc_sequence_id();
        let name = name.trim();
        let name = if name.is_empty() {
            format!("序列 {}", self.sequences.len() + 1)
        } else {
            name.to_string()
        };
        let mut sequence = Sequence::new(name, spec.fps, spec.width, spec.height);
        sequence.id = id;
        self.sequences.push(sequence);
        id
    }

    pub fn rename_sequence(&mut self, id: Id, name: &str) -> Result<(), SequenceError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(SequenceError::EmptyName);
        }
        let sequence = self
            .sequences
            .iter_mut()
            .find(|sequence| sequence.id == id)
            .ok_or(SequenceError::NotFound(id))?;
        sequence.name = name.to_string();
        Ok(())
    }

    /// 删掉一条序列。删的是当前序列时自动切到第一条，播放头与选中一并复位。
    pub fn remove_sequence(&mut self, id: Id) -> Result<(), SequenceError> {
        // 先查 id：点了一个已经不存在的序列时，说「找不到」比「至少留一条」更准。
        let index = self
            .sequences
            .iter()
            .position(|sequence| sequence.id == id)
            .ok_or(SequenceError::NotFound(id))?;
        if self.sequences.len() <= 1 {
            return Err(SequenceError::IsLast);
        }
        self.sequences.remove(index);
        if self.active_sequence == id {
            self.active_sequence = self.sequences[0].id;
            self.reset_view_state();
        }
        Ok(())
    }

    /// 切到另一条序列。播放头、选中与工作区都是「当前序列的位置」，不能带过去；
    /// 缩放与滚动是视图偏好，沿用即可。
    pub fn switch_sequence(&mut self, id: Id) -> Result<(), SequenceError> {
        if !self.sequences.iter().any(|sequence| sequence.id == id) {
            return Err(SequenceError::NotFound(id));
        }
        if self.active_sequence == id {
            return Ok(());
        }
        self.active_sequence = id;
        self.reset_view_state();
        Ok(())
    }

    fn reset_view_state(&mut self) {
        self.ui.playhead = Tick::ZERO;
        self.ui.selected.clear();
        self.ui.clear_range();
    }

    /// 读盘后的自愈：列表非空、id 唯一、指针有效、每个序列内部的排序与计数器都干净。
    ///
    /// 手改过的工程可能把 `sequences` 写成空数组、让两条序列共用 id，或者把
    /// `active_sequence` 指向一条不存在的序列；这些都不该让程序打不开。
    pub(super) fn repair_sequences(&mut self) {
        if self.sequences.is_empty() {
            let id = self.alloc_sequence_id();
            let mut sequence = Sequence::new("序列 1", Rational::FPS_30, 1920, 1080);
            sequence.id = id;
            self.sequences.push(sequence);
        }
        let mut seen: HashSet<Id> = HashSet::new();
        let broken: Vec<usize> = self
            .sequences
            .iter()
            .enumerate()
            .filter(|(_, sequence)| sequence.id.raw() == 0 || !seen.insert(sequence.id))
            .map(|(index, _)| index)
            .collect();
        for index in broken {
            let id = self.alloc_sequence_id();
            self.sequences[index].id = id;
        }
        if !self
            .sequences
            .iter()
            .any(|sequence| sequence.id == self.active_sequence)
        {
            self.active_sequence = self.sequences[0].id;
        }
        let max = self
            .sequences
            .iter()
            .map(|sequence| sequence.id.raw())
            .max()
            .unwrap_or(0);
        self.next_sequence_id = self.next_sequence_id.max(max.saturating_add(1)).max(1);
    }
}
