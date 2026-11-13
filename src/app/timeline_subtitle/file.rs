//! 字幕文件的导入 / 导出（SRT / WebVTT / ASS，M10-T16）。
//!
//! 读写的格式细节在 `timeline::subtitle` 的三个子模块里（`text` 管 SRT / VTT，
//! `ass_script` / `ass_import` 管 ASS）；这里只做三件事：选文件、把结果并成**一步撤销**、
//! 给提示。导入落在已有的第一条字幕轨上，一条字幕轨都没有时新建一条——新建与插入同一个
//! 撤销步，用户按一次 Ctrl+Z 就能整体退掉。
//!
//! SRT / VTT 只有文字与时间，样式给默认值；ASS 连样式一起读回来（字体 / 字号 / 颜色 /
//! 描边 / 位置），导出 ASS 时也把样式写进去，所以「导出再导入」这条路上样式不丢。

use std::path::Path;

use super::*;
use crate::timeline::subtitle::{self, Format, StyledCue, SubtitleStyle, TextCue};
use crate::timeline::{EditError, EditOp, SubtitleItem, TrackKind};

/// 用户挑中的文件按扩展名认出来的三种格式。
///
/// `text::Format` 只覆盖 SRT / VTT（那两种真的只有文字与时间）；ASS 的读写要碰
/// `SubtitleStyle` 与整个序列，走的是 `subtitle::ass_script` / `read_ass`，所以这一层
/// 单独认一遍扩展名，而不是把 `Ass` 塞进 `text::Format` 里。
#[derive(Clone, Copy, PartialEq, Eq)]
enum FileFormat {
    Srt,
    Vtt,
    Ass,
}

impl FileFormat {
    fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "srt" => Some(Self::Srt),
            "vtt" => Some(Self::Vtt),
            "ass" | "ssa" => Some(Self::Ass),
            _ => None,
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Srt => "srt",
            Self::Vtt => "vtt",
            Self::Ass => "ass",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Srt => "SRT",
            Self::Vtt => "WebVTT",
            Self::Ass => "ASS",
        }
    }
}

impl PlayerApp {
    /// 「编辑 > 导入字幕文件…」：SRT / VTT / ASS 各读成一条字幕。
    pub(crate) fn import_subtitle_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.is_none() {
            self.flash("先打开工程，再导入字幕");
            cx.notify();
            return;
        }
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let mut picker = rfd::FileDialog::new()
            .set_title("导入字幕文件")
            .add_filter("字幕（SRT / WebVTT / ASS）", &["srt", "vtt", "ass", "ssa"]);
        if let Some(directory) = self.settings.last_dir.clone() {
            picker = picker.set_directory(directory);
        }
        picker = picker.set_parent(window);

        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(picker.pick_files());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten().unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if !picked.is_empty() {
                    this.read_subtitle_files(picked, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 读文件并并成一步：文件读不出来只计入失败数，别的照常导入。
    fn read_subtitle_files(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let mut cues: Vec<StyledCue> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        for path in &paths {
            self.remember_dir(path);
            match load_cues(path) {
                Ok(mut found) => cues.append(&mut found),
                Err(error) => failed.push(format!("{}：{error}", file_name(path))),
            }
        }
        if cues.is_empty() {
            let detail = if failed.is_empty() {
                "文件里没有能用的字幕块".to_string()
            } else {
                failed.join("；")
            };
            self.flash(format!("没有导入字幕：{detail}"));
            cx.notify();
            return;
        }
        let count = cues.len();
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let mut next = project.sequence().clone();
        // 目标轨：现有的第一条字幕轨；一条都没有就新建（与插入同一步撤销）。
        let track = match next
            .tracks
            .iter()
            .find(|track| track.kind == TrackKind::Subtitle)
            .map(|track| track.id)
        {
            Some(id) => id,
            None => next.add_track(TrackKind::Subtitle),
        };
        let mut skipped = 0usize;
        let mut result = Ok(());
        for cue in cues {
            let mut item = SubtitleItem::new(next.alloc_id(), cue.start, cue.end, cue.text);
            item.style = cue.style;
            item.karaoke = cue.karaoke;
            match crate::timeline::apply(&next, &EditOp::AddSubtitle { track, item }) {
                Ok(applied) => next = applied,
                // 一条轨道上不放重叠的两条：ASS 里同刻双人对话很常见，跳过并计数，
                // 别让一条重叠把整批导入一起丢掉。
                Err(EditError::Overlap(_)) => skipped += 1,
                Err(error) => {
                    result = Err(error);
                    break;
                }
            }
        }
        if let Err(error) = result {
            self.flash(format!("导入字幕失败：{error}"));
            cx.notify();
            return;
        }
        let imported = count - skipped;
        if imported == 0 {
            self.flash(format!("没有导入字幕：{skipped} 条都与已有字幕重叠"));
            cx.notify();
            return;
        }
        let files = paths.len();
        self.commit_edit(next, format!("导入 {imported} 条字幕（{files} 个文件）"));
        let mut message = format!("已导入 {imported} 条字幕");
        if skipped > 0 {
            message.push_str(&format!("，{skipped} 条与已有字幕重叠、没导入"));
        }
        if !failed.is_empty() {
            // 失败的写清楚：用户可能挑到了别的文本格式，或者文件不是 UTF-8。
            message.push_str(&format!(
                "；{} 个文件读不出来（{}）",
                failed.len(),
                failed.join("；")
            ));
        }
        self.flash(message);
        cx.notify();
    }

    /// 「编辑 > 导出字幕文件…」：把序列上的字幕写成 SRT / VTT / ASS。
    pub(crate) fn export_subtitle_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            self.flash("先打开工程，再导出字幕");
            cx.notify();
            return;
        };
        let cues: Vec<TextCue> = subtitle::items(project.sequence())
            .filter(|item| item.end > item.start)
            .map(|item| TextCue {
                start: item.start,
                end: item.end,
                // 用原文而不是 `label()`：多行字幕写出去应该是多行，回来也还是多行。
                text: item.text.clone(),
            })
            .collect();
        if cues.is_empty() {
            self.flash("这条序列上还没有字幕");
            cx.notify();
            return;
        }
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;
        let default_name = format!("{}.srt", project.sequence().name);
        let directory = self.settings.last_dir.clone();

        let mut picker = rfd::FileDialog::new()
            .set_title("导出字幕")
            .add_filter("SRT", &["srt"])
            .add_filter("WebVTT", &["vtt"])
            .add_filter("ASS（带样式）", &["ass"])
            .set_file_name(&default_name);
        if let Some(directory) = directory {
            picker = picker.set_directory(directory);
        }
        picker = picker.set_parent(window);

        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(picker.save_file());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if let Some(path) = picked {
                    this.write_subtitle_file(path, cues, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn write_subtitle_file(&mut self, path: PathBuf, cues: Vec<TextCue>, cx: &mut Context<Self>) {
        // 格式按扩展名认；用户把名字写成别的后缀时补上 .srt，而不是拒绝。
        let format = FileFormat::from_path(&path).unwrap_or(FileFormat::Srt);
        let path = if FileFormat::from_path(&path).is_some() {
            path
        } else {
            path.with_extension(format.extension())
        };
        self.remember_dir(&path);
        let count = cues.len();
        let written = match format {
            // ASS 那份文本就是「序列 → 脚本」，样式与换行都在里面，不走 cue 表。
            FileFormat::Ass => {
                let script = self
                    .project
                    .as_ref()
                    .and_then(|project| subtitle::ass_script(project.sequence(), None));
                match script {
                    Some(script) => std::fs::write(&path, &script).map(|_| script.len()),
                    None => Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "这条序列上还没有字幕",
                    )),
                }
            }
            FileFormat::Srt => subtitle::write(&path, Format::Srt, &cues),
            FileFormat::Vtt => subtitle::write(&path, Format::Vtt, &cues),
        };
        match written {
            Ok(_) => self.flash(format!(
                "已导出 {count} 条字幕（{}）到「{}」",
                format.label(),
                file_name(&path)
            )),
            Err(error) => self.flash(format!("写不了字幕文件：{error}")),
        }
        cx.notify();
    }
}

/// 一份文件读成带样式的字幕：SRT / VTT 只有文字与时间，样式给默认值。
fn load_cues(path: &Path) -> std::io::Result<Vec<StyledCue>> {
    if FileFormat::from_path(path) == Some(FileFormat::Ass) {
        return subtitle::read_ass(path);
    }
    Ok(subtitle::read(path)?
        .into_iter()
        .map(|cue| StyledCue {
            start: cue.start,
            end: cue.end,
            text: cue.text,
            style: SubtitleStyle::default(),
            karaoke: Vec::new(),
        })
        .collect())
}
