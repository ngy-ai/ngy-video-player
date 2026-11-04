use super::super::*;
use crate::remux::{RemuxJob, TrackAction, TrackTag};
use gpui_kit::component::input::InputState;

/// 对话框当前处在哪一步。
#[derive(Clone, Debug)]
pub(super) enum RemuxState {
    Editing,
    Running { progress: f32 },
    Done(PathBuf),
    Failed(String),
}

/// 一条内置轨道的可编辑标签。
///
/// 语言框用组件库的输入框：语言代码虽然是 ASCII，但用户常常是复制粘贴进来的，
/// 自绘输入框没有选区，改错一个字只能整行删掉重打。
pub(super) struct TrackRow {
    /// 轨道名（标题或「语言 · 编码」），只显示。
    pub(super) label: String,
    pub(super) language: Entity<InputState>,
    /// 打开时的语言，用来判断「改过没有」。
    pub(super) original_language: String,
    /// 是否默认轨。同一类轨道里互斥。
    pub(super) default: bool,
    pub(super) original_default: bool,
}

impl TrackRow {
    pub(super) fn edited(&self, cx: &App) -> bool {
        let language = TrackTag::normalized(&self.language.read(cx).value());
        language != self.original_language || self.default != self.original_default
    }
}

/// 轨道编辑对话框的全部状态。
pub(in crate::app) struct RemuxDialog {
    /// 任务代次：后台任务回来时用它确认「这个事件还属于当前对话框」。
    pub(super) generation: u64,
    /// 这次编辑的是音轨还是字幕。
    pub(super) target: EditTarget,
    pub(super) path: PathBuf,
    pub(super) duration: f64,
    /// 原文件这一路的内置轨道摘要，只用来显示「内置几条」。
    pub(super) builtin: Vec<String>,
    /// 原文件内置字幕轨的编码名。编辑音轨时也要带上：字幕按原样保留，
    /// 但保留成什么编码得由它的原编码决定。
    pub(super) subtitle_codecs: Vec<String>,
    /// 当前这一路每条内置轨道的语言与默认标记，顺序与输出一致。
    pub(super) rows: Vec<TrackRow>,
    pub(super) action: TrackAction,
    pub(super) state: RemuxState,
    pub(super) job: Option<Arc<RemuxJob>>,
}

impl RemuxDialog {
    pub(super) fn new(
        generation: u64,
        target: EditTarget,
        path: PathBuf,
        snapshot: &Snapshot,
        window: &mut Window,
        cx: &mut Context<PlayerApp>,
    ) -> Self {
        // 外挂字幕不算「内置」：它们不在容器里，ffmpeg 根本看不到，写进摘要只会误导。
        let kind = match target {
            EditTarget::Audio => TrackKind::Audio,
            EditTarget::Subtitle => TrackKind::Subtitle,
        };
        let tracks: Vec<_> = snapshot
            .track(kind)
            .into_iter()
            .filter(|track| !track.external)
            .collect();
        let builtin = tracks
            .iter()
            .map(|track| track.display_name())
            .collect::<Vec<_>>();
        let subtitle_codecs = snapshot
            .track(TrackKind::Subtitle)
            .into_iter()
            .filter(|track| !track.external)
            .map(|track| track.codec.clone())
            .collect();
        let rows = tracks
            .iter()
            .map(|track| {
                let language = TrackTag::normalized(&track.lang);
                let language_input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .default_value(language.clone())
                        .placeholder("chi")
                });
                TrackRow {
                    label: track.display_name(),
                    language: language_input,
                    original_language: language,
                    default: track.default,
                    original_default: track.default,
                }
            })
            .collect();

        Self {
            generation,
            target,
            path,
            duration: snapshot.duration,
            builtin,
            subtitle_codecs,
            rows,
            action: TrackAction::Keep,
            state: RemuxState::Editing,
            job: None,
        }
    }

    pub(super) fn editing(&self) -> bool {
        matches!(self.state, RemuxState::Editing | RemuxState::Failed(_))
    }

    /// 保持原样、标签也没改时「开始」没有意义：这不是「复制一份文件」的入口。
    pub(super) fn changed(&self, cx: &App) -> bool {
        if !self.action.is_keep() {
            return true;
        }
        self.rows.iter().any(|row| row.edited(cx))
    }

    /// 表单里的语言与默认标记。任何一条的语言不合法都整体拒绝——
    /// 写进容器的语言代码错了，播放器只会显示一串乱码，事后很难发现。
    pub(super) fn tags(&self, cx: &App) -> Result<Vec<TrackTag>, String> {
        self.rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let text = row.language.read(cx).value();
                // 空字符串是「不写语言」，允许。
                if !text.trim().is_empty() && !TrackTag::valid_language(&text) {
                    return Err(format!(
                        "第 {} 条轨道的语言代码「{}」不对：写 2–3 个字母，例如 chi、eng、jpn",
                        index + 1,
                        text.trim()
                    ));
                }
                Ok(TrackTag {
                    language: TrackTag::normalized(&text),
                    default: row.default,
                })
            })
            .collect()
    }

    /// 交给 ffmpeg 的两路处置：只有当前这一路可能被改，另一路永远原样保留。
    pub(super) fn actions(&self) -> (TrackAction, TrackAction) {
        match self.target {
            EditTarget::Audio => (self.action.clone(), TrackAction::Keep),
            EditTarget::Subtitle => (TrackAction::Keep, self.action.clone()),
        }
    }
}

/// 按钮文案：(选文件, 删除, 恢复)。
pub(super) fn labels(target: EditTarget) -> (&'static str, &'static str, &'static str) {
    match target {
        EditTarget::Audio => ("选择音频文件…", "删除音轨", "恢复原音轨"),
        EditTarget::Subtitle => ("选择字幕文件…", "移除字幕", "恢复原字幕"),
    }
}

/// 卡片底部的说明。
pub(super) fn hint(target: EditTarget) -> &'static str {
    match target {
        EditTarget::Audio => {
            "输出为同目录下的新 mkv：画面和字幕直接复制，换上的音轨保持原编码。\
             原文件不会被改动。"
        }
        EditTarget::Subtitle => {
            "输出为同目录下的新 mkv：画面和音轨直接复制，换上的字幕会转成容器支持的编码。\
             原文件不会被改动。"
        }
    }
}
