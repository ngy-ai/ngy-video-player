//! 导出时间线：把 `timeline::ExportPlan` 跑成文件。
//!
//! 计划本身在 `timeline::export` 里，是纯字符串、不碰进程；这里负责起 ffmpeg、
//! 读进度、取消时删掉半成品。放在 `timeline` 外面是因为它依赖 `platform::ffmpeg`，
//! 而那一层的约束是「只依赖 std 与 serde」。

mod task;

pub use task::{ExportEvent, ExportTask};
