//! 时间线：从播放器走向剪辑器的那一半。
//!
//! 这里只有**没有窗口也能测试**的部分：时间基、序列模型、编辑纯函数、工程文件。
//! 架构约束见 `TODO.md` §0.5：本模块只依赖 `std` 与 `serde`，不许 import `app`、
//! `ui`、`platform`；界面通过读状态、发命令来驱动它，需要回传时走事件。
//!
//! 时间单位唯一：时间点与时长一律 [`Tick`]（`i64` 微秒），帧率一律 [`Rational`]。
//! 禁止用 `f64` 秒做累加、比较或存盘——错一帧的 bug 在时间线上无法调试。

pub mod autosave;
mod export;
mod media;
mod model;
mod ops;
pub mod pack;
pub mod preview;
mod project;
mod spec;
mod time;
mod undo;
mod view;

pub use export::{ExportError, ExportPlan, plan_sequence};
pub use media::{AssetState, ColorTags, MediaInfo, MediaStamp};
pub use model::{
    AssetId, ClipItem, Id, Marker, ModelError, Sequence, Track, TrackKind, Transition,
    TransitionKind,
};
pub use ops::{ClipSpec, EditError, EditOp, apply, remove_clips, split_at};
pub use project::{
    Asset, AssetRemoveError, PROJECT_EXTENSION, PROJECT_VERSION, Project, ProjectError, UiState,
};
pub use spec::{SequenceSpec, SpecError, rate_label};
pub use time::{MICROS_PER_SECOND, Rational, Tick};
pub use undo::{HISTORY_LIMIT, UndoHistory};
pub use view::{
    MIN_ZOOM, ViewState, frame_pixels, playhead_limit, seek_seconds, snap_to_targets,
    visible_frames, visible_ticks,
};
