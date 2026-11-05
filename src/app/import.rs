//! 把媒体文件导入时间线：登记素材、探测、接进序列。
//!
//! 单个文件优先落在播放头，撞上了就退到末尾。来源有两种：文件选择框，以及
//! 「从播放列表导入」——后者先选一页列表，再选其中一个视频，一次只导一个。

use std::path::Path;

use super::*;
use crate::timeline::{AssetId, ClipSpec, EditOp, MediaInfo, TrackKind, apply as apply_edit};

/// 「从播放列表导入」对话框：先选一页列表，再选其中的一个视频。
pub(super) struct PlaylistImportDialog {
    /// 当前显示的是哪一页。
    tab: usize,
    /// 选中的文件在 `Playlist::items` 里的下标；只可能是文件行。
    selected: Option<usize>,
}

impl PlayerApp {
    /// 菜单「导入到时间线…」：选文件，插到播放头。
    pub(super) fn import_to_timeline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.is_none() || self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let mut picker = rfd::FileDialog::new()
            .add_filter("视频", formats::VIDEO)
            .add_filter("音频", formats::AUDIO)
            .set_title("导入到时间线")
            .set_parent(window);
        if let Some(directory) = self.settings.last_dir.clone() {
            picker = picker.set_directory(directory);
        }

        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(picker.pick_files());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if let Some(paths) = picked {
                    this.import_paths(paths, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 菜单「从播放列表导入…」。
    ///
    /// 工程模式下右侧播放列表不画，所以用应用内对话框重新给一份：先选列表，
    /// 再选视频。一次只导入一个，误点最多多一段，撤一步就回去了。
    pub(super) fn open_playlist_import_dialog(&mut self, cx: &mut Context<Self>) {
        if self.project.is_none() {
            return;
        }
        // 再点一次菜单就是收起：和裁切面板的开关语义一致。
        if self.playlist_import.is_some() {
            self.close_playlist_import_dialog(cx);
            return;
        }
        let has_files = self
            .playlists
            .tabs()
            .iter()
            .any(|tab| tab.playlist.file_count() > 0);
        if !has_files {
            self.flash("播放列表里还没有文件");
            cx.notify();
            return;
        }

        // 这几个对话框都是「对工程做件大事」的，同一时间只留一个。
        self.close_remux_dialog(cx);
        self.close_metadata_dialog(cx);
        self.close_proxy_dialog(cx);
        self.close_scope_dialog(cx);
        self.close_scenes_dialog(cx);
        self.close_multicam_dialog(cx);
        self.close_concat_dialog(cx);

        let tab = self.playlists.active();
        // 默认选这一页正在播的那一项：多半就是想把它放进时间线。
        let selected = self.playlists.tab(tab).and_then(Playlist::index);
        self.playlist_import = Some(PlaylistImportDialog { tab, selected });
        self.menu_bar = None;
        cx.notify();
    }

    pub(super) fn close_playlist_import_dialog(&mut self, cx: &mut Context<Self>) {
        if self.playlist_import.take().is_some() {
            cx.notify();
        }
    }

    /// 换一页列表：预选这一页的当前项，没有就空着，等用户自己点。
    fn select_import_tab(&mut self, tab: usize, cx: &mut Context<Self>) {
        let Some(list) = self.playlists.tab(tab) else {
            return;
        };
        let selected = list.index();
        if let Some(dialog) = self.playlist_import.as_mut() {
            dialog.tab = tab;
            dialog.selected = selected;
        }
        cx.notify();
    }

    fn select_import_item(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(dialog) = self.playlist_import.as_mut() else {
            return;
        };
        if !self
            .playlists
            .tab(dialog.tab)
            .is_some_and(|list| list.playable_path(index).is_some())
        {
            return;
        }
        dialog.selected = Some(index);
        cx.notify();
    }

    /// 把选中的那一个文件插进时间线。
    fn confirm_playlist_import(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.playlist_import.take() else {
            return;
        };
        let Some(path) = self.playlists.tab(dialog.tab).and_then(|list| {
            dialog
                .selected
                .and_then(|index| list.playable_path(index).cloned())
        }) else {
            return;
        };
        self.import_paths(vec![path], cx);
    }

    pub(super) fn render_playlist_import_dialog(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.playlist_import.as_ref()?;
        let list = self.playlists.tab(dialog.tab)?;

        // 第一级：选列表。页签多了换行，按钮区不跟着无限变高。
        let tabs = div()
            .id("playlist-import-tabs")
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.0))
            .max_h(px(76.0))
            .overflow_y_scroll()
            .children(
                self.playlists
                    .tabs()
                    .iter()
                    .enumerate()
                    .map(|(index, tab)| {
                        let active = index == dialog.tab;
                        div()
                            .id(gpui_kit::ElementId::Name(
                                format!("playlist-import-tab-{index}").into(),
                            ))
                            .px(px(10.0))
                            .py(px(4.0))
                            .rounded(px(6.0))
                            .cursor(CursorStyle::PointingHand)
                            .text_size(px(11.0))
                            .bg(if active { ui::accent() } else { ui::track() })
                            .text_color(if active { bg() } else { text_muted() })
                            .child(ui::ellipsis(tab.name.clone()))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                cx.stop_propagation();
                                this.select_import_tab(index, cx);
                            }))
                    }),
            );

        // 第二级：选视频。只列文件行，文件夹不占位置。
        let items = div()
            .id("playlist-import-items")
            .flex()
            .flex_col()
            .gap(px(2.0))
            .max_h(px(300.0))
            .overflow_y_scroll()
            .children(
                list.items()
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| !item.is_dir())
                    .map(|(index, item)| {
                        let selected = dialog.selected == Some(index);
                        div()
                            .id(gpui_kit::ElementId::Name(
                                format!("playlist-import-item-{index}").into(),
                            ))
                            .px(px(10.0))
                            .py(px(7.0))
                            .rounded(px(6.0))
                            .cursor(CursorStyle::PointingHand)
                            .text_size(px(12.0))
                            .text_color(if selected { ui::accent() } else { text() })
                            .when(selected, |this| this.bg(gpui_kit::rgba(0x5aa9ff1f)))
                            .when(!selected, |this| {
                                this.hover(|style| style.bg(ui::surface_hover()))
                            })
                            .child(ui::ellipsis(file_name(item.path())))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                cx.stop_propagation();
                                this.select_import_item(index, cx);
                            }))
                    }),
            );

        let confirm_enabled = dialog
            .selected
            .and_then(|index| list.playable_path(index))
            .is_some();

        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(text_muted())
                    .child("先选一页列表，再选一个视频；导入后插在播放头，撞上了排到末尾。"),
            )
            .child(tabs);

        if list.file_count() == 0 {
            body = body.child(
                div()
                    .py(px(24.0))
                    .flex()
                    .justify_center()
                    .text_size(px(12.0))
                    .text_color(text_faint())
                    .child("这一页还没有可导入的文件"),
            );
        } else {
            body = body.child(items);
        }

        let body = body.child(
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(8.0))
                .child(settings_button(
                    "playlist-import-cancel",
                    "取消",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.close_playlist_import_dialog(cx);
                    }),
                ))
                .child(primary_button(
                    "playlist-import-confirm",
                    "导入",
                    confirm_enabled,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.confirm_playlist_import(cx);
                    }),
                )),
        );

        Some(modal_card(
            "playlist-import-dialog",
            "从播放列表导入",
            body.into_any_element(),
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.close_playlist_import_dialog(cx);
            }),
        ))
    }

    /// 文件选择框那一路：同步探测，逐个插进时间线。
    ///
    /// 多选时整批接在序列末尾；只选一个文件时优先落在播放头，撞上了再退到末尾。
    fn import_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let batch = paths.len() > 1;
        let mut cursor = self
            .project
            .as_ref()
            .map(|project| project.sequence.duration());
        let mut imported = 0;
        let mut failed = 0;

        for path in &paths {
            let candidates: Vec<Tick> = if batch {
                cursor.into_iter().collect()
            } else {
                let playhead = self
                    .project
                    .as_ref()
                    .map(|project| project.ui.playhead)
                    .unwrap_or(Tick::ZERO);
                let end = self
                    .project
                    .as_ref()
                    .map(|project| project.sequence.duration())
                    .unwrap_or(Tick::ZERO);
                vec![playhead, end]
            };
            if candidates.is_empty() {
                failed += 1;
                continue;
            }
            match self.insert_media(path, &candidates) {
                Some(end) => {
                    cursor = Some(end);
                    imported += 1;
                }
                None => failed += 1,
            }
        }

        // 整批算一步撤销：按一次「导入」却要按 N 次撤销才能退回去太别扭。
        if imported > 0 {
            self.remember_history();
        }
        self.report_import(imported, failed);
        self.poke();
        cx.notify();
    }

    /// 登记并插入一个文件。`candidates` 是按顺序尝试的起点，第一个放得下就用。
    ///
    /// 返回插入后的终点；没插进去返回 `None`。
    fn insert_media(&mut self, path: &Path, candidates: &[Tick]) -> Option<Tick> {
        let project = self.project.as_mut()?;
        let id = match crate::media::register(project, path) {
            Ok(id) => id,
            Err(error) => {
                log_warn!("导入「{}」失败：{error:#}", path.display());
                return None;
            }
        };
        // 先拿到素材信息再改序列：`asset` 的借用不能跨过 `sequence` 的可变借用。
        let media = self
            .project
            .as_ref()?
            .asset(id)
            .and_then(|asset| asset.media.clone())?;
        if media.duration <= Tick::ZERO {
            log_warn!("「{}」读不出时长，跳过", path.display());
            return None;
        }

        candidates
            .iter()
            .find_map(|at| self.insert_probed(id, &media, *at))
    }

    /// 把探测好的素材插到 `at`。所有目标轨道都放得下才算成功。
    ///
    /// 视频与音频用同一个起点：分成两次各找位置会让声音和画面对不齐。
    fn insert_probed(&mut self, id: AssetId, media: &MediaInfo, at: Tick) -> Option<Tick> {
        let project = self.project.as_mut()?;
        let targets = target_tracks(&project.sequence, media.has_video(), media.has_audio());
        if targets.is_empty() {
            return None;
        }

        // 先在副本上排完，全部成功才写回：插一半失败会留下没有声音的画面。
        let mut next = project.sequence.clone();
        for track in targets {
            let spec = ClipSpec::new(id, Tick::ZERO, media.duration);
            match apply_edit(
                &next,
                &EditOp::Insert {
                    track,
                    at,
                    clip: spec,
                },
            ) {
                Ok(sequence) => next = sequence,
                Err(error) => {
                    let name = project
                        .asset_path(id)
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| format!("素材 {id:?}"));
                    log_warn!("「{name}」放不进时间线：{error}");
                    return None;
                }
            }
        }

        project.sequence = next;
        Some(at + media.duration)
    }

    fn report_import(&mut self, imported: usize, failed: usize) {
        if imported > 0 {
            self.flash(format!("已导入 {imported} 个素材"));
        }
        if failed > 0 {
            self.error = Some(Banner::new(
                "有素材没有导入",
                "这些文件可能不是视频/音频、已经损坏，或者没有可用的轨道。详细原因见日志。",
            ));
        }
    }
}

/// 一个素材该落到哪些轨道：有画面进视频轨、有声音进音频轨，锁定的轨道跳过。
fn target_tracks(
    sequence: &crate::timeline::Sequence,
    has_video: bool,
    has_audio: bool,
) -> Vec<crate::timeline::Id> {
    sequence
        .tracks
        .iter()
        .filter(|track| !track.locked)
        .filter(|track| match track.kind {
            TrackKind::Video => has_video,
            TrackKind::Audio => has_audio,
            TrackKind::Subtitle => false,
        })
        .map(|track| track.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{Rational, Sequence};

    #[test]
    fn 素材按有无画面与声音分配到轨道() {
        let sequence = Sequence::new("测试", Rational::FPS_25, 1920, 1080);
        let video = sequence.tracks[0].id;
        let audio = sequence.tracks[1].id;

        assert_eq!(target_tracks(&sequence, true, true), vec![video, audio]);
        assert_eq!(target_tracks(&sequence, true, false), vec![video]);
        assert_eq!(target_tracks(&sequence, false, true), vec![audio]);
        assert!(target_tracks(&sequence, false, false).is_empty());
    }

    #[test]
    fn 锁定的轨道不收素材() {
        let mut sequence = Sequence::new("测试", Rational::FPS_25, 1920, 1080);
        let video = sequence.tracks[0].id;
        sequence.track_mut(video).unwrap().locked = true;
        assert!(target_tracks(&sequence, true, false).is_empty());
    }
}
