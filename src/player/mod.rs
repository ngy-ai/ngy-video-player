//! 播放引擎。

pub mod engine;
// release 构建内嵌进可执行文件的 libmpv：缺库时由 ffi 调用来解压。
pub(crate) mod embed;
// 只供 engine / probe 内部使用：对外暴露会迫使调用方持有不透明的原始句柄，
// 安全边界留在引擎线程里维护。
pub(crate) mod ffi;
// 主画面与预览缩略图共用的软件渲染缓冲。
pub(crate) mod frame;
pub mod probe;
// 进度条悬停预览（独立的第二个 mpv 实例）。
pub(crate) mod thumbnail;

pub use engine::{
    Chapter, Command, EndReason, Engine, EngineOptions, LoopMode, PlayerEvent, Snapshot, Track,
    TrackKind,
};
pub use probe::{MediaInfo, MediaTrack};
