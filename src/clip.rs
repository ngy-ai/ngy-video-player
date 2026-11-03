//! 视频裁切：把 `[开始, 结束]` 区间导出成新文件。
//!
//! 两端都可以留空：开始留空表示从头，结束留空表示到最后（见 `range`）。导出参数、
//! 滤镜链与 ffmpeg 参数在 `export`；单文件任务在 `job`，多段任务在 `cutout`，
//! 拼接在 `concat`；产物命名在 `naming`，对外类型在 `types`。
//!
//! 本文件只放模块声明与对外出口，具体实现按职责分在同名子目录里。

mod concat;
mod cutout;
mod encoder;
mod export;
mod job;
mod keyframe;
mod loudness;
mod naming;
mod preset;
mod range;
mod types;

pub use concat::{ConcatEvent, ConcatJob, ConcatOutcome, ConcatRequest};
pub use export::{
    ColorMatrix, ColorRange, Crop, DEFAULT_BITRATE_KBPS, DEFAULT_SPLIT_MB, Edge, EncoderMode,
    ExportMode, ExportOptions, FrameRate, KeyframeAlign, MAX_FREEZE_SECONDS, OutputFormat, Pad,
    PixelFormat, Quality, RateControl, Rotate, Scale, VideoCodec, VideoTransform, Watermark,
    WatermarkPosition,
};
pub use job::ClipJob;
pub use loudness::Loudness;
pub use naming::{concat_output_path, cut_out_output_path, output_path, split_output_paths};
pub use preset::{ExportPreset, PresetStore, normalize_name};
pub use range::{
    ClipRange, ParsedTime, RangeError, format_time, parse_time, parse_time_with_fps,
    range_from_inputs, range_from_inputs_with_fps,
};
pub use types::{BurnSubtitle, ClipEvent, ClipOutcome, ClipRequest, CutKind};

// 只在裁切内部使用：`cutout` 判断空片段、`job` 判断关键帧是否撞上终点。
pub(crate) use range::MIN_DURATION;
