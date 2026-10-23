//! 播放引擎。

pub mod engine;
// 只供 engine 内部使用：对外暴露会迫使调用方持有不透明的原始句柄，
// 安全边界留在引擎线程里维护。
pub(crate) mod ffi;

pub use engine::{
    Chapter, Command, EndReason, Engine, LoopMode, PlayerEvent, Snapshot, Track, TrackKind,
};
