//! 时间线：从播放器走向剪辑器的那一半。
//!
//! 这里只有**没有窗口也能测试**的部分：时间基、序列模型、编辑纯函数、工程文件。
//! 架构约束见 `TODO.md` §0.5：本模块只依赖 `std` 与 `serde`，不许 import `app`、
//! `ui`、`platform`；界面通过读状态、发命令来驱动它，需要回传时走事件。
//!
//! 时间单位唯一：时间点与时长一律 [`Tick`]（`i64` 微秒），帧率一律 [`Rational`]。
//! 禁止用 `f64` 秒做累加、比较或存盘——错一帧的 bug 在时间线上无法调试。

pub mod autosave;
mod color;
mod effect;
mod export;
mod keyframe;
mod media;
mod model;
mod ops;
pub mod pack;
pub mod preview;
mod project;
mod spec;
mod speed;
mod subtitle;
mod three_point;
mod time;
mod transition;
mod undo;
mod view;

pub use color::{COLOR_RANGE, COLOR_STEP, ColorGrade};
pub use effect::{
    Effect, EffectKind, EffectParams, Knob, add_effect, cycle_knob, filter_chain as effect_chain,
    interpolated_fps, move_effect, remove_effect, sequence_fps, toggle_effect, unknown_effects,
};
pub use export::{
    EncodeSpeed, ExportError, ExportPlan, ExportSettings, PRESET_NAME_CHARS, Quality, RenderPreset,
    RenderPresetStore, SubtitleAttachment, SubtitleMode, VideoCodec,
    normalize_name as normalize_preset_name, plan_sequence, plan_sequence_id, plan_sequence_range,
    plan_sequence_with,
};
pub use keyframe::{
    Animation, Interpolation, Keyframe, Keyframes, Property, fade_filters as keyframe_fades,
    motion_filter as keyframe_motion,
};
pub use media::{AssetState, ColorTags, MediaInfo, MediaStamp};
pub use model::{
    AssetId, ClipItem, Id, Marker, ModelError, Sequence, Track, TrackKind, Transition,
    TransitionKind,
};
pub use ops::{ClipSpec, EditError, EditOp, Placement, apply, remove_clips, split_at};
pub use project::{
    Asset, AssetRemoveError, PROJECT_EXTENSION, PROJECT_VERSION, Project, ProjectError,
    SequenceError, UiState,
};
pub use spec::{SequenceSpec, SpecError, rate_label};
pub use speed::{set_hold, set_speed};
pub use subtitle::{
    COLORS, FONTS, MAX_OUTLINE, MAX_SIZE, MIN_SIZE, POSITIONS, SIZE_STEP, SubtitleItem,
    SubtitlePosition, SubtitleStyle, ass_script as subtitle_ass_script, at as subtitle_at,
    items as subtitle_items,
};
pub use three_point::{ThreePoint, ThreePointError, three_point};
pub use time::{MICROS_PER_SECOND, Rational, Tick};
pub use transition::{
    DEFAULT_TRANSITION, MAX_TRANSITION, MIN_TRANSITION, TransitionApplied, TransitionError,
    current as transition_at, remove as remove_transition, set as set_transition, transition_room,
};
pub use undo::{HISTORY_LIMIT, UndoHistory};
pub use view::{
    MIN_ZOOM, ViewState, frame_pixels, playhead_limit, seek_seconds, snap_to_targets,
    visible_frames, visible_ticks,
};
