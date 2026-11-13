//! 字幕文件的导入 / 导出（SRT / WebVTT，M10-T16）。
//!
//! 读写的格式细节在 `timeline::subtitle::text`；这里只做三件事：选文件、把结果并成
//! **一步撤销**、给提示。导入落在已有的第一条字幕轨上，一条字幕轨都没有时新建一条
//! ——新建与插入同一个撤销步，用户按一次 Ctrl+Z 就能整体退掉。

use super::*;
use crate::timeline::subtitle::{self, Format, TextCue};
use crate::timeline::{EditOp, SubtitleItem, TrackKind};

impl PlayerApp {
    /// 「编辑 > 导入字幕文件…」：SRT / VTT 各读成一条字幕。
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
            .add_filter("字幕（SRT / WebVTT）", &["srt", "vtt"]);
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
        let mut cues: Vec<TextCue> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        for path in &paths {
            self.remember_dir(path);
            match subtitle::read(path) {
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
        let mut result = Ok(());
        for cue in cues {
            let item = SubtitleItem::new(next.alloc_id(), cue.start, cue.end, cue.text);
            match crate::timeline::apply(&next, &EditOp::AddSubtitle { track, item }) {
                Ok(applied) => next = applied,
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
        let files = paths.len();
        self.commit_edit(next, format!("导入 {count} 条字幕（{files} 个文件）"));
        if failed.is_empty() {
            self.flash(format!("已导入 {count} 条字幕"));
        } else {
            // 失败的写清楚：用户可能挑到了 ASS，或者文件不是 UTF-8。
            self.flash(format!(
                "已导入 {count} 条字幕，{} 个文件读不出来（{}）",
                failed.len(),
                failed.join("；")
            ));
        }
        cx.notify();
    }

    /// 「编辑 > 导出字幕文件…」：把序列上的字幕写成 SRT / VTT。
    pub(crate) fn export_subtitle_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            self.flash("先打开工程，再导出字幕");
            cx.notify();
            return;
        };
        let cues: Vec<TextCue> = subtitle::items(project.sequence())
            .map(|item| TextCue {
                start: item.start,
                end: item.end,
                text: item.label(),
            })
            .filter(|cue| cue.end > cue.start)
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
        let format = Format::from_path(&path).unwrap_or(Format::Srt);
        let path = if Format::from_path(&path).is_some() {
            path
        } else {
            path.with_extension(format.extension())
        };
        self.remember_dir(&path);
        let count = cues.len();
        match subtitle::write(&path, format, &cues) {
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
