//! 撤销栈：撤销到空、重做到底、分支操作、上限与无变化不入栈。

use super::*;
use crate::timeline::{AssetId, ClipSpec, Id, Rational};

fn seq() -> Sequence {
    Sequence::new("测试", Rational::FPS_25, 1920, 1080)
}

fn s(n: i64) -> Tick {
    Tick::from_micros(n * 1_000_000)
}

fn insert(track: Id, at: Tick, duration: Tick) -> EditOp {
    EditOp::Insert {
        track,
        at,
        clip: ClipSpec::new(AssetId::new(1), Tick::ZERO, duration),
    }
}

#[test]
fn 撤销到空再重做到底() {
    let mut history = UndoHistory::new(seq());
    let track = history.current().tracks[0].id;

    history.edit(&insert(track, s(0), s(1))).expect("插入");
    history.edit(&insert(track, s(2), s(2))).expect("插入");
    assert_eq!(history.current().tracks[0].clips.len(), 2);
    assert_eq!(history.undo_steps(), 2);
    assert_eq!(history.redo_steps(), 0);
    assert_eq!(history.duration(), s(4));

    assert_eq!(history.undo().unwrap().tracks[0].clips.len(), 1);
    assert_eq!(history.undo().unwrap().tracks[0].clips.len(), 0);
    assert!(history.undo().is_none(), "已经是初始状态");
    assert!(!history.can_undo());
    assert_eq!(history.redo_steps(), 2);

    assert_eq!(history.redo().unwrap().tracks[0].clips.len(), 1);
    assert_eq!(history.redo().unwrap().tracks[0].clips.len(), 2);
    assert!(history.redo().is_none());
    assert!(!history.can_redo());
}

#[test]
fn 撤销之后做新操作会清空重做分支() {
    let mut history = UndoHistory::new(seq());
    let track = history.current().tracks[0].id;
    history.edit(&insert(track, s(0), s(1))).expect("插入");
    history.edit(&insert(track, s(2), s(1))).expect("插入");

    history.undo().expect("撤销一步");
    assert!(history.can_redo());

    history.edit(&insert(track, s(10), s(1))).expect("新操作");
    assert!(!history.can_redo(), "分支被丢掉");
    assert_eq!(history.undo_steps(), 2);

    let clips = &history.current().tracks[0].clips;
    assert_eq!(clips.len(), 2);
    assert_eq!(clips[0].start, s(0), "没有被撤销掉的那一步还在");
    assert_eq!(clips[1].start, s(10));
}

#[test]
fn 栈满丢最旧的一步() {
    let mut history = UndoHistory::with_limit(seq(), 3);
    let track = history.current().tracks[0].id;
    for index in 0..5 {
        history.edit(&insert(track, s(index), s(1))).expect("插入");
    }

    assert_eq!(history.undo_steps(), 3, "最多留三步");
    while history.undo().is_some() {}
    assert_eq!(
        history.current().tracks[0].clips.len(),
        2,
        "最早的两步已经被丢掉，撤不回空序列"
    );
}

#[test]
fn 上限为零时只保留当前状态() {
    let mut history = UndoHistory::with_limit(seq(), 0);
    let track = history.current().tracks[0].id;
    history.edit(&insert(track, s(0), s(1))).expect("插入");

    assert_eq!(history.current().tracks[0].clips.len(), 1);
    assert_eq!(history.undo_steps(), 0);
    assert!(history.undo().is_none());
}

#[test]
fn 状态没变不占一步() {
    let mut history = UndoHistory::new(seq());
    let snapshot = history.current().clone();
    assert!(!history.push(snapshot));
    assert_eq!(history.undo_steps(), 0);
}

#[test]
fn 失败的操作不动历史() {
    let mut history = UndoHistory::new(seq());
    let before = history.current().clone();
    let result = history.edit(&EditOp::Move {
        clip: Id::new(999),
        to_track: before.tracks[0].id,
        start: Tick::ZERO,
    });

    assert!(result.is_err());
    assert_eq!(history.current(), &before);
    assert!(!history.can_undo());
}

#[test]
fn 一次编辑只记一步() {
    let mut history = UndoHistory::new(seq());
    let track = history.current().tracks[0].id;
    let clip = {
        let next = history.edit(&insert(track, s(0), s(4))).expect("插入");
        next.tracks[0].clips[0].id
    };

    // 拖动过程中界面会反复算落点，但只在松手时入栈；这里模拟一次完整的拖拽。
    let start = {
        let moved = history
            .edit(&EditOp::Move {
                clip,
                to_track: track,
                start: s(3),
            })
            .expect("移动");
        moved.tracks[0].clips[0].start
    };
    assert_eq!(start, s(3));
    assert_eq!(history.undo_steps(), 2, "插入一步、拖拽一步");

    history.undo().expect("撤销拖拽");
    assert_eq!(
        history.current().tracks[0].clips[0].start,
        s(0),
        "拖拽整步回退"
    );
}

#[test]
fn 打开新工程时重置历史() {
    let mut history = UndoHistory::new(seq());
    let track = history.current().tracks[0].id;
    history.edit(&insert(track, s(0), s(1))).expect("插入");
    assert!(history.can_undo());

    history.reset(seq());
    assert!(!history.can_undo());
    assert!(!history.can_redo());
    assert_eq!(history.current().tracks[0].clips.len(), 0);
    assert_eq!(history.labels().collect::<Vec<_>>(), ["初始状态"]);
}

#[test]
fn 说明与状态一起入栈并随分支裁剪() {
    let mut history = UndoHistory::new(seq());
    let track = history.current().tracks[0].id;
    let next = apply(history.current(), &insert(track, s(0), s(1))).expect("插入");
    history.push_labeled(next, "导入素材");

    assert_eq!(
        history.labels().collect::<Vec<_>>(),
        ["初始状态", "导入素材"]
    );
    assert_eq!(history.current_index(), 1);

    // 撤销后开新分支：旧说明不能留在新状态后面。
    history.undo().expect("撤销");
    let next = apply(history.current(), &insert(track, s(3), s(1))).expect("插入");
    history.push_labeled(next, "切开片段");
    assert_eq!(
        history.labels().collect::<Vec<_>>(),
        ["初始状态", "切开片段"]
    );
}

#[test]
fn 跳到指定步既能回退也能前进() {
    let mut history = UndoHistory::new(seq());
    let track = history.current().tracks[0].id;
    for index in 0..3 {
        let next = apply(history.current(), &insert(track, s(index), s(1))).expect("插入");
        history.push_labeled(next, format!("第 {} 步", index + 1));
    }
    assert_eq!(history.current_index(), 3);

    assert_eq!(history.jump_to(1).unwrap().tracks[0].clips.len(), 1);
    assert_eq!(history.current_index(), 1);
    assert_eq!(history.redo_steps(), 2, "后面的步骤还留着");

    assert_eq!(history.jump_to(3).unwrap().tracks[0].clips.len(), 3);
    assert_eq!(history.current_index(), 3);
    assert!(history.jump_to(3).is_none(), "停在原地不算一次操作");
    assert!(history.jump_to(99).is_none(), "越界不算一次操作");
}

#[test]
fn 栈满时说明跟着最旧一步一起丢() {
    let mut history = UndoHistory::with_limit(seq(), 2);
    let track = history.current().tracks[0].id;
    for index in 0..4 {
        let next = apply(history.current(), &insert(track, s(index), s(1))).expect("插入");
        history.push_labeled(next, format!("步骤 {index}"));
    }
    assert_eq!(
        history.labels().collect::<Vec<_>>(),
        ["步骤 1", "步骤 2", "步骤 3"],
        "最旧的状态连同它的说明一起被丢掉"
    );
}
