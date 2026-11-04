use super::super::*;
use crate::clip::{self as clip_core, ParsedTime};
use crate::metadata::{ChapterEdit, MetadataJob, Tags};
use gpui_kit::component::input::InputState;

/// 对话框当前处在哪一步。
#[derive(Clone, Debug)]
pub(super) enum MetadataState {
    Editing,
    Running { progress: f32 },
    Done(PathBuf),
    Failed(String),
}

/// 一条可编辑的章节。
pub(super) struct ChapterRow {
    /// 起点文本，接受 `90`、`1:30`、`01:02:03.500`；保存时才解析。
    /// 用输入框而不是只读文本，是为了能直接改时间码修章节。
    pub(super) start: Entity<InputState>,
    pub(super) title: Entity<InputState>,
}

/// 打开对话框时从播放器快照里抄出来的一份文件信息。
///
/// 单独成结构是为了让 `MetadataDialog::new` 只收两个参数：数据一批、窗口与上下文一批。
pub(super) struct MetadataSource<'a> {
    pub(super) path: PathBuf,
    pub(super) duration: f64,
    /// 片源帧率，用来换算 SMPTE 时间码里的帧号。0 表示还没读出来。
    pub(super) fps: f64,
    pub(super) tags: &'a Tags,
    pub(super) chapters: Vec<(String, f64)>,
    /// 原文件内置字幕的编码名，保存时决定字幕是复制还是转码。
    pub(super) subtitle_codecs: Vec<String>,
}

/// 对话框的全部状态。
pub(in crate::app) struct MetadataDialog {
    pub(super) generation: u64,
    pub(super) path: PathBuf,
    pub(super) duration: f64,
    /// 片源帧率，用来换算 SMPTE 时间码里的帧号。
    pub(super) fps: f64,
    pub(super) title: Entity<InputState>,
    pub(super) artist: Entity<InputState>,
    pub(super) comment: Entity<InputState>,
    pub(super) chapters: Vec<ChapterRow>,
    /// 原文件内置字幕的编码名，保存时决定字幕是复制还是转码。
    pub(super) subtitle_codecs: Vec<String>,
    /// 新选的封面图。`None` 表示沿用原文件的封面（原文件没有就不写）。
    pub(super) cover: Option<PathBuf>,
    pub(super) state: MetadataState,
    pub(super) job: Option<Arc<MetadataJob>>,
}

impl MetadataDialog {
    pub(super) fn new(
        generation: u64,
        source: MetadataSource<'_>,
        window: &mut Window,
        cx: &mut Context<PlayerApp>,
    ) -> Self {
        let title = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(source.tags.title.clone())
                .placeholder("留空则删除标题")
        });
        let artist = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(source.tags.artist.clone())
                .placeholder("留空则删除作者")
        });
        let comment = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(source.tags.comment.clone())
                .placeholder("留空则删除备注")
        });

        let mut chapters: Vec<(String, f64)> = source.chapters;
        chapters.sort_by(|a, b| a.1.total_cmp(&b.1));
        let chapters = chapters
            .into_iter()
            .map(|(title, start)| {
                let start_text = clip_core::format_time(start);
                ChapterRow {
                    start: cx.new(|cx| {
                        InputState::new(window, cx)
                            .default_value(start_text)
                            .placeholder("00:00.000")
                    }),
                    title: cx.new(|cx| {
                        InputState::new(window, cx)
                            .default_value(title)
                            .placeholder("章节标题")
                    }),
                }
            })
            .collect();

        Self {
            generation,
            path: source.path,
            duration: source.duration,
            fps: source.fps,
            title,
            artist,
            comment,
            chapters,
            subtitle_codecs: source.subtitle_codecs,
            cover: None,
            state: MetadataState::Editing,
            job: None,
        }
    }

    pub(super) fn editing(&self) -> bool {
        matches!(
            self.state,
            MetadataState::Editing | MetadataState::Failed(_)
        )
    }

    /// 当前表单里的标签。空字符串表示删除该标签。
    pub(super) fn tags(&self, cx: &App) -> Tags {
        Tags {
            title: self.title.read(cx).value().trim().to_string(),
            artist: self.artist.read(cx).value().trim().to_string(),
            comment: self.comment.read(cx).value().trim().to_string(),
        }
    }

    /// 解析所有时间框。任何一条不合法都整体拒绝：带着错的章节去写文件，
    /// 用户会得到一个「保存成功但章节不对」的结果，比直接报错更难查。
    pub(super) fn chapter_edits(
        &self,
        cx: &App,
        duration: f64,
    ) -> Result<Vec<ChapterEdit>, String> {
        self.chapters
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let text = row.start.read(cx).value();
                let start = match clip_core::parse_time_with_fps(&text, self.fps) {
                    ParsedTime::Value(start) if start < duration => start,
                    ParsedTime::Value(_) => {
                        return Err(format!(
                            "第 {} 条章节超出片长（{}）",
                            index + 1,
                            clip_core::format_time(duration)
                        ));
                    }
                    ParsedTime::Empty => {
                        return Err(format!("第 {} 条章节的时间是空的", index + 1));
                    }
                    ParsedTime::Invalid => {
                        return Err(format!(
                            "第 {} 条章节的时间格式不对：{}（可写 90、1:30、01:02:03.500、01:02:03;15）",
                            index + 1,
                            text
                        ));
                    }
                };
                Ok(ChapterEdit {
                    title: row.title.read(cx).value().trim().to_string(),
                    start,
                })
            })
            .collect()
    }
}
