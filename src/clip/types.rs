//! 裁切任务的对外类型：请求、事件、结果，以及「这次要做什么」。
//!
//! 界面只用这几样东西跟后台任务打交道：发一个请求、收进度与结果、拿着句柄取消。
//! 单独成文件是为了让 `clip.rs` 只留组装逻辑，也方便别的模块引用而不拖进 ffmpeg 细节。

use super::loudness::Loudness;
use super::{ClipRange, ExportOptions};
use std::path::PathBuf;

/// 要烧进画面的内置字幕轨。
///
/// 只有文本字幕能烧：点阵字幕（PGS / VobSub）是图片，`subtitles` 滤镜没有文本
/// 可画，只能继续作为软字幕保留。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BurnSubtitle {
    /// 容器里的第几条字幕流，对应 `-map 0:s:N`。
    pub stream: usize,
    /// 给界面显示的轨道名。
    pub name: String,
}

/// 这次裁切要做什么。
///
/// 三种操作共用同一套区间输入与导出参数，只是区间的含义不同：保留、抠掉、
/// 或者拿来当切点。分成枚举而不是几个布尔量，省得同时勾上两个互相打架的开关。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CutKind {
    /// 只保留区间。
    #[default]
    Keep,
    /// 去掉区间，保留前后两段并拼成一个文件。
    RemoveMiddle,
    /// 在区间起点切成两个文件。
    Split,
}

impl CutKind {
    /// 产物是一个还是两个。
    pub fn output_count(self) -> usize {
        match self {
            CutKind::Split => 2,
            _ => 1,
        }
    }
}

/// 一次裁切请求。
#[derive(Clone, Debug)]
pub struct ClipRequest {
    pub input: PathBuf,
    pub output: PathBuf,
    pub range: ClipRange,
    pub options: ExportOptions,
    /// 烧录哪一条内置字幕；`None` 表示不烧。
    pub burn: Option<BurnSubtitle>,
    /// 两遍响度的第一遍结果。界面不填，任务在启动时测出来写进去；
    /// `None` 表示走单遍 loudnorm。
    pub loudness: Option<Loudness>,
    /// 这次要做什么。`RemoveMiddle` 与 `Split` 都会强制重编码。
    pub kind: CutKind,
    /// 源文件总时长。只有「删除中间段」需要——后半段要一直留到片尾。
    pub duration: f64,
}

/// 后台任务回报给界面的事件。
#[derive(Clone, Debug)]
pub enum ClipEvent {
    /// 0.0–1.0。
    Progress(f32),
    Done(ClipOutcome),
}

#[derive(Clone, Debug)]
pub enum ClipOutcome {
    /// 产物可能是多个（分割会写出两个文件），按写出顺序排列。
    Finished(Vec<PathBuf>),
    Cancelled,
    Failed(String),
}
