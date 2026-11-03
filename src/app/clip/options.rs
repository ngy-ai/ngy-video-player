//! 裁切面板里的高级导出选项：模块入口。
//!
//! 选项全部收在面板里而不是设置窗口：它们只对这一次导出有效，不该变成跨会话的
//! 偏好。用户选完、点「开始」、关掉面板，下一次回到默认值。
//!
//! 按主题拆开：剪切方式与输出格式在 `kind`，编码/码率/像素格式在 `encoding`，
//! 画面变换与处理在 `picture`，时间戳与轨道保留在 `tracks`，步进器一类的小控件
//! 在 `controls`；音频那一段在 `audio_options`。这里只负责按顺序拼装。

mod controls;
mod encoding;
mod kind;
mod picture;
mod presets;
mod tracks;

use super::*;
use crate::clip::ExportOptions;

impl PlayerApp {
    /// 改一项导出参数。面板开着才生效，改完立刻重绘。
    pub(super) fn update_clip_options(
        &mut self,
        update: impl FnOnce(&mut ExportOptions),
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.clip.as_mut() else {
            return;
        };
        update(&mut dialog.options);
        cx.notify();
    }

    /// 全部高级选项。每一块自己判断该不该出现，没有内容时整块收掉——
    /// 空块留在 flex 里会多出一段 gap，面板看起来像掉了一块。
    pub(super) fn render_clip_options(
        &self,
        dialog: &ClipDialog,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let blocks: Vec<AnyElement> = [
            // 预设摆最上面：先挑一个现成的再微调，是这套面板最常见的顺序。
            Some(self.render_clip_presets(dialog, cx)),
            self.render_clip_kind(dialog, cx),
            self.render_clip_format(dialog, cx),
            self.render_clip_encoding(dialog, cx),
            self.render_clip_picture(dialog, cx),
            self.render_clip_tracks(dialog, cx),
        ]
        .into_iter()
        .flatten()
        .collect();

        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .children(blocks)
            .into_any_element()
    }
}
