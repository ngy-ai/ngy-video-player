//! 工程模式预览：播放头落在哪段素材上，就把那一刻的画面送进播放器。
//!
//! 取帧规则在 `timeline::preview`（纯函数，和导出共用一份选轨逻辑）。这里只做
//! 「怎么喂给 mpv」这一件事。
//!
//! 同一个文件只 seek，换文件才 `Load`：换片有几百毫秒的加载，拖播放头时每挪几个
//! 像素就换一次片，画面会一顿一顿地黑。

use std::path::Path;

use super::*;

impl PlayerApp {
    /// 按当前播放头刷新预览画面。
    pub(super) fn preview_at_playhead(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let at = project.ui.playhead;
        let Some(frame) = crate::timeline::preview::resolve(&project.sequence, at) else {
            // 空档：没有画面可给。留着上一帧，比突然黑屏更像「这里什么都没有」。
            return;
        };
        let Some(path) = project.asset_path(frame.asset).map(Path::to_path_buf) else {
            return;
        };
        let source = frame.source.secs();

        if self.timeline_preview.as_deref() != Some(path.as_path()) {
            self.timeline_preview = Some(path.clone());
            self.send(Command::Load {
                url: path.to_string_lossy().into_owned(),
                start: Some(source),
            });
            // 拖播放头要看的是「这一帧」，不是从这儿接着放。
            self.send(Command::SetPaused(true));
        } else {
            self.send(Command::SeekAbsolute(source));
        }
        self.poke();
        cx.notify();
    }
}
