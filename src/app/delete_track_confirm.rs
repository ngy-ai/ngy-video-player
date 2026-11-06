//! 删除轨道前的确认框：轨道上还有内容时先问一句。
//!
//! 空轨道直接删；有片段的轨道删掉会连片段和转场一起消失，虽然能撤销，误点一下
//! 整段素材从时间线上不见还是太吓人。视频轨与音频轨共用这一个入口。

use super::*;
use crate::timeline::{Id, TrackKind};

/// 正在等用户回答的那条轨道。
///
/// 只记 id 和当时的摘要：确认期间序列可能又变了（撤销、拖入新素材），真正删除时
/// 重新按 id 查一次，别拿「当时的片段数」当删除依据。
#[derive(Clone, Debug)]
pub(super) struct DeleteTrackConfirm {
    track: Id,
    kind: TrackKind,
    name: String,
    detail: String,
}

/// 轨道上有没有会跟着一起被删掉的内容。
///
/// 只有转场没有片段属于坏数据（`Sequence::validate` 会拒），但删轨不该因为数据
/// 不干净就跳过确认：宁可多问一句。
pub(super) fn track_has_content(track: &crate::timeline::Track) -> bool {
    !track.clips.is_empty() || !track.transitions.is_empty()
}

/// 轨道内容的中文摘要。片段数是最直观的那一项。
pub(super) fn track_content_detail(track: &crate::timeline::Track) -> String {
    if track.clips.is_empty() {
        "还有转场记录".to_string()
    } else {
        format!("还有 {} 个片段", track.clips.len())
    }
}

impl PlayerApp {
    /// 打开确认框。其它模态都收掉，免得两层遮罩叠在一起抢键盘。
    ///
    /// 裁切 / 转换是右侧面板，工程模式下本来就不画，也别去关它们——那会顺手
    /// 取消正在跑的后台任务。
    pub(super) fn open_delete_track_confirm(
        &mut self,
        track: Id,
        kind: TrackKind,
        name: String,
        detail: String,
        cx: &mut Context<Self>,
    ) {
        self.close_remux_dialog(cx);
        self.close_metadata_dialog(cx);
        self.close_proxy_dialog(cx);
        self.close_scope_dialog(cx);
        self.close_scenes_dialog(cx);
        self.close_multicam_dialog(cx);
        self.close_concat_dialog(cx);
        self.close_playlist_import_dialog(cx);
        self.show_settings = false;
        self.file_info = None;
        self.tab_rename = None;
        self.timeline_menu = None;
        self.menu = None;
        self.menu_bar = None;

        self.delete_track_confirm = Some(DeleteTrackConfirm {
            track,
            kind,
            name,
            detail,
        });
        cx.notify();
    }

    /// 取消 / × / Esc：什么都不删。
    pub(super) fn close_delete_track_confirm(&mut self, cx: &mut Context<Self>) {
        if self.delete_track_confirm.take().is_some() {
            cx.notify();
        }
    }

    /// 确认：按 id 重新找一次轨道再删；期间轨道已被删掉时 `delete_track_now` 自己会停。
    pub(super) fn confirm_delete_track(&mut self, cx: &mut Context<Self>) {
        let Some(confirm) = self.delete_track_confirm.take() else {
            return;
        };
        self.delete_track_now(confirm.track, cx);
    }

    /// 确认框打开时独占键盘：Esc 取消、Enter 确认，其余键一律吞掉，
    /// 免得空格 / 方向键漏到播放器快捷键上。
    pub(super) fn on_delete_track_confirm_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.delete_track_confirm.is_none() {
            return false;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.close_delete_track_confirm(cx),
            "enter" => self.confirm_delete_track(cx),
            _ => {}
        }
        true
    }

    pub(super) fn render_delete_track_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let confirm = self.delete_track_confirm.as_ref()?;
        let description = format!(
            "{}轨「{}」上{}。删除会连同片段和转场一起移除。",
            confirm.kind.label(),
            confirm.name,
            confirm.detail
        );

        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(text_muted())
                    .whitespace_normal()
                    .child(description),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(text_faint())
                    .whitespace_normal()
                    .child("删错了可以撤销，或在「编辑历史」侧栏里回到之前那一步。"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.0))
                    .child(settings_button(
                        "delete-track-cancel",
                        "取消",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.close_delete_track_confirm(cx);
                        }),
                    ))
                    .child(primary_button(
                        "delete-track-confirm",
                        "删除轨道",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.confirm_delete_track(cx);
                        }),
                    )),
            )
            .into_any_element();

        Some(modal_card(
            "delete-track-confirm",
            "删除轨道？",
            body,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.close_delete_track_confirm(cx);
            }),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{AssetId, ClipItem, Rational, Sequence, Tick};

    #[test]
    fn 空轨道不用确认有片段或转场才要() {
        let mut sequence = Sequence::new("测试", Rational::FPS_25, 1920, 1080);
        let track_id = sequence.tracks[0].id;
        assert!(!track_has_content(&sequence.tracks[0]));

        let clip_id = sequence.alloc_id();
        sequence
            .track_mut(track_id)
            .unwrap()
            .clips
            .push(ClipItem::new(
                clip_id,
                AssetId::new(1),
                Tick::ZERO,
                Tick::from_secs(2.0),
                Tick::ZERO,
            ));
        assert!(track_has_content(&sequence.tracks[0]));
        assert_eq!(track_content_detail(&sequence.tracks[0]), "还有 1 个片段");
    }
}
