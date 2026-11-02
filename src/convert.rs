//! 格式转换：把当前文件、或者整个文件夹（含子文件夹）里的媒体转成指定格式。
//!
//! 与裁切共用同一套 ffmpeg 调用方式（后台线程 + `-progress` + 杀进程取消），差别在
//! 批次：一个文件夹可能有几十上百个文件，这里一次只跑一个——同时开几个编码器只会
//! 让每一个都变慢，还会把磁盘写满。
//!
//! 输出落在源文件同目录。目标文件已存在（包括「就是源文件自己」）时改名成
//! `xxx - 转换.mkv` 并往后编号，绝不覆盖原片。

mod format;
mod scan;
mod task;

// 编码器偏好与裁切共用同一套定义，这里重新导出，批次调用方不必再多引一个模块。
pub use crate::encoder::EncoderMode;
pub use format::{TargetFormat, TargetKind};
pub use scan::scan_folder;
pub use task::{
    ConvertEvent, ConvertFailure, ConvertJob, ConvertOutcome, ConvertRequest, ConvertSummary, plan,
};
