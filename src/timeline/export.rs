//! 序列导出：把时间线展开成 ffmpeg 的输入、滤镜图与输出参数。
//!
//! 这一层只产出参数、不启动进程：参数是纯字符串，能用单测钉住；进程与进度留给
//! 后续任务。第一版只处理「一条视频轨 + 一条音频轨」，多轨叠加与转场在后面加。

mod plan;
mod presets;
mod settings;

use std::fmt;
use std::path::PathBuf;

pub use plan::{plan_sequence, plan_sequence_range, plan_sequence_with};
pub use presets::{PRESET_NAME_CHARS, RenderPreset, RenderPresetStore, normalize_name};
pub use settings::{EncodeSpeed, ExportSettings, Quality, VideoCodec};

/// 一条导出计划：喂给 ffmpeg 的全部素材与滤镜。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportPlan {
    /// 参与编码的素材文件，按首次出现顺序去重。
    pub inputs: Vec<PathBuf>,
    /// `-filter_complex` 的内容。
    pub filter: String,
    /// 视频流的滤镜输出标签，作为 `-map` 的目标。
    pub video_label: Option<String>,
    /// 音频流的滤镜输出标签。
    pub audio_label: Option<String>,
    /// 输出参数（不含输出文件本身）。
    pub output_args: Vec<String>,
}

impl ExportPlan {
    /// 拼成完整的 ffmpeg 参数（不含 `ffmpeg` 命令本身）。
    ///
    /// 输出文件不在这里：调用方要在参数末尾追加目标路径，中途失败时才好换名重写。
    pub fn args(&self) -> Vec<String> {
        let mut args = Vec::new();
        for input in &self.inputs {
            args.push("-i".to_string());
            args.push(input.to_string_lossy().into_owned());
        }
        if !self.filter.is_empty() {
            args.push("-filter_complex".to_string());
            args.push(self.filter.clone());
        }
        for label in [&self.video_label, &self.audio_label].into_iter().flatten() {
            args.push("-map".to_string());
            args.push(label.clone());
        }
        args.extend(self.output_args.iter().cloned());
        args
    }
}

/// 生成不出计划的原因。都是「问用户要东西」的错误，不是内部 bug。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportError {
    /// 时间线上一个片段都没有。
    Empty,
    /// 没有可用的视频轨（没有视频轨，或者视频轨全被隐藏）。
    NoVideoTrack,
    /// 片段引用的素材不在素材表里。
    UnknownAsset(u64),
    /// 素材文件在打开工程时就已经找不到了。
    MissingAsset(String),
    /// 工作区（入点 / 出点）夹到序列范围之后是空的。
    EmptyRange,
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "时间线上还没有片段"),
            Self::NoVideoTrack => write!(f, "没有启用的视频轨，先在轨道名旁把眼睛打开"),
            Self::UnknownAsset(raw) => write!(f, "片段引用的素材 {raw} 不在素材表里"),
            Self::MissingAsset(name) => {
                write!(f, "素材「{name}」找不到了，先把它放回原处或重新链接")
            }
            Self::EmptyRange => write!(f, "工作区是空的：入点要早于出点"),
        }
    }
}

impl std::error::Error for ExportError {}

#[cfg(test)]
mod tests;
