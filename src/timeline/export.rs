//! 序列导出：把时间线展开成 ffmpeg 的输入、滤镜图与输出参数。
//!
//! 这一层只产出参数、不启动进程：参数是纯字符串，能用单测钉住；进程与进度留给
//! 后续任务。第一版只处理「一条视频轨 + 一条音频轨」，多轨叠加与转场在后面加。

mod plan;
mod presets;
mod settings;
mod stems;

use std::fmt;
use std::path::{Path, PathBuf};

pub use plan::{plan_sequence, plan_sequence_id, plan_sequence_range, plan_sequence_with};

impl ExportPlan {
    /// 把遮罩占位输入换成真实路径（任务层写好 PGM 之后调用）。
    pub fn resolve_masks(&mut self, paths: &[PathBuf]) {
        for (attachment, path) in self.masks.iter().zip(paths) {
            if let Some(slot) = self.inputs.get_mut(attachment.input) {
                *slot = path.clone();
            }
        }
    }
}
pub use presets::{PRESET_NAME_CHARS, RenderPreset, RenderPresetStore, normalize_name};
pub use settings::{EncodeSpeed, ExportSettings, Quality, SubtitleMode, VideoCodec};
pub use stems::{StemPlan, plan_stems};

/// 导出计划里附带的字幕：一份 ASS 文本 + 交付方式。
///
/// 这里只带文本不带路径：计划是「入队即定死」的纯数据，临时 `.ass` 文件要到任务
/// 真正启动时才写（队列里排着的两条导出不能抢同一个文件名）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubtitleAttachment {
    pub ass: String,
    pub mode: SubtitleMode,
}

/// 导出计划里附带的一张遮罩 alpha 图。
///
/// 与字幕同一个道理：这里只带字节不带路径。`input` 是它在 [`ExportPlan::inputs`]
/// 里的位置（生成计划时占好位），任务启动时才把 PGM 写到临时目录并替换那个路径。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MaskAttachment {
    pub input: usize,
    pub pgm: Vec<u8>,
}

/// 一条导出计划：喂给 ffmpeg 的全部素材与滤镜。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportPlan {
    /// 参与编码的素材文件，按首次出现顺序去重。遮罩图也在这里（见 `masks`）。
    pub inputs: Vec<PathBuf>,
    /// `-filter_complex` 的内容。
    pub filter: String,
    /// 视频流的滤镜输出标签，作为 `-map` 的目标。
    pub video_label: Option<String>,
    /// 音频流的滤镜输出标签。
    pub audio_label: Option<String>,
    /// 输出参数（不含输出文件本身）。
    pub output_args: Vec<String>,
    /// 序列上的字幕条目。没有字幕时是 `None`，命令里也不多一个输入。
    pub subtitle: Option<SubtitleAttachment>,
    /// 带遮罩的片段：片段 id → 那张 alpha 图落在哪个输入位上。没有遮罩时是空的。
    pub masks: Vec<MaskAttachment>,
}

impl ExportPlan {
    /// 拼成完整的 ffmpeg 参数（不含 `ffmpeg` 命令本身）。
    ///
    /// 输出文件不在这里：调用方要在参数末尾追加目标路径，中途失败时才好换名重写。
    /// 字幕走 [`Self::args_with_subtitle`]：临时文件是任务层才有的东西，计划本身保持
    /// 纯数据，既有的字符串快照也不会因为临时路径变来变去。
    pub fn args(&self) -> Vec<String> {
        self.build(None)
    }

    /// 带上已落盘的 ASS 文件拼参数。`codec` 由调用方按输出容器挑（mkv 用 `ass`、
    /// mp4 用 `mov_text`）；烧录时这个文件只是 `subtitles` 滤镜的输入，不做 `-i`。
    pub fn args_with_subtitle(&self, path: &Path, mode: SubtitleMode, codec: &str) -> Vec<String> {
        self.build(Some((path, mode, codec)))
    }

    fn build(&self, subtitle: Option<(&Path, SubtitleMode, &str)>) -> Vec<String> {
        // 软字幕是额外一路输入。追加在末尾，前面那些 `[N:v]` / `[N:a]` 的下标不变。
        let soft_input = match subtitle {
            Some((path, SubtitleMode::Soft, _)) => Some(path),
            _ => None,
        };
        let mut args = Vec::new();
        for input in self.inputs.iter().map(PathBuf::as_path).chain(soft_input) {
            args.push("-i".to_string());
            args.push(input.to_string_lossy().into_owned());
        }

        // 烧录排在滤镜图的最后：字幕要盖在转场、效果、调色都算完的成片上。
        let mut filter = self.filter.clone();
        let mut video_label = self.video_label.clone();
        if let Some((path, SubtitleMode::Burn, _)) = subtitle
            && let Some(label) = video_label.clone()
        {
            if !filter.is_empty() {
                filter.push(';');
            }
            filter.push_str(&format!(
                "{label}subtitles=filename='{}'[subv]",
                super::color::filter_path(path)
            ));
            video_label = Some("[subv]".to_string());
        }
        if !filter.is_empty() {
            args.push("-filter_complex".to_string());
            args.push(filter);
        }
        for label in [&video_label, &self.audio_label].into_iter().flatten() {
            args.push("-map".to_string());
            args.push(label.clone());
        }
        if let Some((_, SubtitleMode::Soft, codec)) = subtitle {
            let index = self.inputs.len();
            args.extend([
                "-map".to_string(),
                format!("{index}:s:0"),
                "-c:s".to_string(),
                codec.to_string(),
            ]);
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
    /// 没有可导出的音频轨（没有音频轨、全是静音，或者全被独奏挤掉）。
    NoAudioTrack,
    /// 片段引用的素材不在素材表里。
    UnknownAsset(u64),
    /// 素材文件在打开工程时就已经找不到了。
    MissingAsset(String),
    /// 素材本身就是代理文件（`*.proxy.mp4`）：导出必须用原片。
    ProxySource(String),
    /// 工作区（入点 / 出点）夹到序列范围之后是空的。
    EmptyRange,
    /// 记下要导的序列之后，它被从工程里删掉了。
    SequenceGone,
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "时间线上还没有片段"),
            Self::NoVideoTrack => write!(f, "没有启用的视频轨，先在轨道名旁把眼睛打开"),
            Self::NoAudioTrack => {
                write!(f, "没有可导出的音频轨：空轨、静音轨与被独奏挤掉的轨都不算")
            }
            Self::UnknownAsset(raw) => write!(f, "片段引用的素材 {raw} 不在素材表里"),
            Self::MissingAsset(name) => {
                write!(f, "素材「{name}」找不到了，先把它放回原处或重新链接")
            }
            Self::ProxySource(name) => write!(
                f,
                "素材「{name}」是代理文件（*.proxy.mp4）：代理只给预览用，导出必须换成原片"
            ),
            Self::EmptyRange => write!(f, "工作区是空的：入点要早于出点"),
            Self::SequenceGone => write!(f, "要导的序列已经被删掉了，重新打开导出面板再选一次"),
        }
    }
}

impl std::error::Error for ExportError {}

#[cfg(test)]
mod tests;
