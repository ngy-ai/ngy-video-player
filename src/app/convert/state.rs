//! 格式转换面板的状态：来源、扫描结果、目标格式与批次进度。

use super::super::*;
use crate::convert::{ConvertJob, ConvertSummary, TargetFormat};

/// 这一批文件从哪儿来。
#[derive(Clone, Debug)]
pub(super) enum ConvertSource {
    /// 正在播放的那一个文件。
    ///
    /// 刻意不存路径：面板不挡播放，用户随时可能换片，跟着换成新的那一个才符合直觉；
    /// 存下旧路径会把「刚才那个文件」重新转一遍，而屏幕上放着的是别的片子。
    Current,
    Folder(PathBuf),
}

/// 文件夹扫描的结果。
#[derive(Clone, Debug)]
pub(super) enum ScanState {
    /// 当前文件模式，不需要扫描。
    Idle,
    Scanning,
    Ready(Vec<PathBuf>),
    Failed(String),
}

/// 面板当前停在哪一步。
#[derive(Clone, Debug)]
pub(super) enum ConvertState {
    Editing,
    Running {
        /// 正在处理的第几项（0 起）。
        index: usize,
        total: usize,
        /// 当前文件名，进度条旁边要显示它。
        name: String,
        /// 当前项的进度；总时长读不到时是 `None`。
        ratio: Option<f32>,
    },
    Done {
        summary: ConvertSummary,
        cancelled: bool,
    },
    Failed(String),
}

/// 目标格式下拉列表的展开状态。
///
/// 六十多种格式铺成网格要滚好几屏，所以收进一个可搜索的下拉里：默认只显示
/// 当前选中的那一个，点开才铺列表，输入即筛选。
#[derive(Default)]
pub(super) struct FormatPicker {
    pub(super) open: bool,
    /// 搜索词。点开时清空，关掉也清空——下次打开从完整列表开始。
    pub(super) query: String,
}

/// 转换面板的全部状态。
pub(in crate::app) struct ConvertDialog {
    /// 任务代次：后台任务与扫描结果回来时用它确认「事件还属于当前面板」。
    pub(super) generation: u64,
    pub(super) source: ConvertSource,
    pub(super) target: TargetFormat,
    pub(super) scan: ScanState,
    pub(super) state: ConvertState,
    pub(super) job: Option<Arc<ConvertJob>>,
    pub(super) picker: FormatPicker,
}

impl ConvertDialog {
    pub(super) fn new(generation: u64) -> Self {
        Self {
            generation,
            source: ConvertSource::Current,
            // 默认 mp4：最常见的「发给别人也能播」的格式。
            target: TargetFormat::default(),
            scan: ScanState::Idle,
            state: ConvertState::Editing,
            job: None,
            picker: FormatPicker::default(),
        }
    }

    /// 现在允许改来源 / 格式、重新开始。
    pub(super) fn editing(&self) -> bool {
        matches!(self.state, ConvertState::Editing | ConvertState::Failed(_))
    }

    /// 格式下拉是否展开。Esc 的分支顺序要按它判断，所以在模块外也得看得见。
    pub(in crate::app) fn picker_open(&self) -> bool {
        self.picker.open
    }

    /// 当前搜索词命中的格式，顺序与格式表一致（常用的在前）。
    pub(super) fn matches(&self) -> Vec<TargetFormat> {
        TargetFormat::ALL
            .iter()
            .copied()
            .filter(|format| format.matches(&self.picker.query))
            .collect()
    }
}
