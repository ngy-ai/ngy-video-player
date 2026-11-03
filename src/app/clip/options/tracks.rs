//! 时间戳与轨道保留：修复时间戳、保留音轨、烧录字幕、字幕/章节是否写回。
//!
//! 这一块对流复制同样有效（轨道是 `-map` 决定的，与编不编码无关），所以它不在
//! 「只在重编码时出现」的分支里。

use super::super::widgets::{note, note_owned, section_title, toggle_row};
use super::super::*;
use crate::clip::OutputFormat;

impl PlayerApp {
    pub(super) fn render_clip_tracks(
        &self,
        dialog: &ClipDialog,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let options = &dialog.options;
        let gif = options.format == OutputFormat::Gif;

        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(section_title("时间戳"))
            .child(toggle_row(
                "clip-option-fix-pts",
                "修复缺失/负时间戳",
                options.fix_timestamps,
                |this, cx| {
                    this.update_clip_options(|options| options.fix_timestamps ^= true, cx);
                },
                cx,
            ))
            .child(note("对流复制也生效；音频漂移校正只在重编码时可用。"))
            .child(section_title("轨道"));

        // GIF 没有音轨可留，这个开关留着只会让人以为能导出带声音的动图。
        if !gif {
            body = body.child(toggle_row(
                "clip-option-audio",
                "保留音轨",
                options.keep_audio,
                |this, cx| {
                    this.update_clip_options(|options| options.keep_audio ^= true, cx);
                },
                cx,
            ));
        }

        // 烧录只对文本字幕有意义；这个文件没有可烧的轨道时，这一行不出现。
        // 批处理里每个文件的内置字幕轨都不一样，任务侧不做烧录，这里也就不露开关。
        if let Some(subtitle) = dialog.subtitle.as_ref().filter(|_| !dialog.batch()) {
            body = body
                .child(toggle_row(
                    "clip-option-burn-subtitle",
                    "烧录内置字幕（硬字幕）",
                    dialog.burn_subtitle,
                    |this, cx| this.toggle_clip_burn(cx),
                    cx,
                ))
                .child(note_owned(format!(
                    "把「{}」画进画面，烧进去就再也关不掉；原字幕轨不再复制",
                    subtitle.name
                )));
        }

        if options.format == OutputFormat::Mkv {
            // 烧录之后软字幕不再复制，这个开关留着只会让人以为还能同时保留两份。
            if !dialog.burn_subtitle {
                body = body.child(toggle_row(
                    "clip-option-subtitles",
                    "保留字幕",
                    options.keep_subtitles,
                    |this, cx| {
                        this.update_clip_options(|options| options.keep_subtitles ^= true, cx);
                    },
                    cx,
                ));
            }
            body = body.child(toggle_row(
                "clip-option-chapters",
                "章节与元数据",
                options.keep_chapters,
                |this, cx| {
                    this.update_clip_options(|options| options.keep_chapters ^= true, cx);
                },
                cx,
            ));
        } else if gif {
            body = body.child(note("GIF 只包含画面：没有声音，也没有字幕与章节。"));
        } else if options.format.is_split() {
            body = body.child(note("每段都是独立 mkv，字幕与章节照常保留。"));
        } else {
            body = body.child(note("切片输出只保留画面与音轨，字幕与章节不写入。"));
        }

        Some(body.into_any_element())
    }
}
