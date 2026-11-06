//! 把媒体文件导入时间线：登记素材、探测、接进序列。
//!
//! 单个文件优先落在播放头，撞上了就退到末尾。来源有两种：文件选择框，以及
//! 「从播放列表导入」——后者先选一页列表，再选其中一个视频，一次只导一个。

use std::path::Path;

use super::*;
use crate::timeline::{AssetId, ClipSpec, EditOp, MediaInfo, TrackKind, apply as apply_edit};
use gpui_kit::Subscription;
use gpui_kit::component::input::{Input, InputState};

/// 「从播放列表导入」对话框：先选一页列表，再选其中的一个视频。
pub(super) struct PlaylistImportDialog {
    /// 当前显示的是哪一页。
    tab: usize,
    /// 选中的文件在 `Playlist::items` 里的下标；只可能是文件行。
    selected: Option<usize>,
    /// 搜索框。文件名多是中文，必须走组件库输入框：自绘输入框没有输入法。
    search: Entity<InputState>,
    /// 搜索框当前的文本。输入框只会重绘自己，靠观察者同步到这里来过滤列表。
    query: String,
    /// 搜索词变化后清掉旧选择、重绘文件列表。
    _search_changes: Subscription,
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
    pub(super) fn open_playlist_import_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        // 默认选这一页正在播的那一项：多半就是想把它放进时间线。它藏在收起的
        // 文件夹里时不预选，免得按钮亮着却看不见选中的是哪一行。
        let selected = self.playlists.tab(tab).and_then(|list| {
            let visible = list.visible_indices();
            list.index().filter(|index| visible.contains(index))
        });

        let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索文件名"));
        let search_changes = cx.observe(&search, |this, state, cx| {
            let query = state.read(cx).value().to_string();
            let Some(dialog) = this.playlist_import.as_mut() else {
                return;
            };
            // 光标移动也会触发通知，只有文本真的变了才重绘列表。
            if dialog.query == query {
                return;
            }
            // 搜索词变了：旧选中可能已不在结果里，别让「导入」指着一行
            // 界面上看不见的文件。
            dialog.query = query;
            dialog.selected = None;
            cx.notify();
        });

        self.playlist_import = Some(PlaylistImportDialog {
            tab,
            selected,
            search,
            query: String::new(),
            _search_changes: search_changes,
        });
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
        let searching = self
            .playlist_import
            .as_ref()
            .is_some_and(|dialog| !dialog.query.trim().is_empty());
        // 搜索中不预选：当前项多半不在结果里，非要选上只会让「导入」指着一行
        // 界面上看不见的文件。
        let selected = if searching {
            None
        } else {
            let visible = list.visible_indices();
            list.index().filter(|index| visible.contains(index))
        };
        if let Some(dialog) = self.playlist_import.as_mut() {
            dialog.tab = tab;
            dialog.selected = selected;
        }
        cx.notify();
    }

    /// 展开或收起一个文件夹。收起来之后选中的文件看不见了，顺手取消选择：
    /// 确认按钮不该指着一个界面上根本不存在的行。
    fn toggle_import_dir(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(tab) = self.playlist_import.as_ref().map(|dialog| dialog.tab) else {
            return;
        };
        let Some(list) = self.playlists.tab_mut(tab) else {
            return;
        };
        if !list.toggle_dir(index) {
            return;
        }
        let visible = list.visible_indices();
        if let Some(dialog) = self.playlist_import.as_mut()
            && dialog
                .selected
                .is_some_and(|selected| !visible.contains(&selected))
        {
            dialog.selected = None;
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

        let searching = !dialog.query.trim().is_empty();
        // 第二级：选视频。列表是树，文件夹照样画出来：点文件夹展开/收起，
        // 点文件才是选中。搜索时只留匹配的文件和它们的祖先，忽略折叠状态。
        let rows = list.search_indices(&dialog.query);
        let no_match = searching && rows.is_empty();
        let items = div()
            .id("playlist-import-items")
            .flex()
            .flex_col()
            .gap(px(1.0))
            .max_h(px(300.0))
            .overflow_y_scroll()
            .children(rows.into_iter().map(|index| {
                let Some(item) = list.items().get(index) else {
                    return div().into_any_element();
                };
                let is_dir = item.is_dir();
                let folded = item.collapsed();
                let depth = item.depth();
                let selected = !is_dir && dialog.selected == Some(index);
                div()
                    .id(gpui_kit::ElementId::Name(
                        format!("playlist-import-item-{index}").into(),
                    ))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    // 缩进就是层级：和面板上的树保持同一套节奏。
                    .pl(px(10.0 + f32::from(depth) * PLAYLIST_INDENT))
                    .pr(px(10.0))
                    .py(px(6.0))
                    .rounded(px(6.0))
                    // 搜索结果的文件夹只是层级提示，点了不会改变结果，别给手型。
                    .when(!is_dir || !searching, |this| {
                        this.cursor(CursorStyle::PointingHand)
                    })
                    .text_size(px(12.0))
                    .text_color(if selected {
                        ui::accent()
                    } else if is_dir {
                        text_muted()
                    } else {
                        text()
                    })
                    .when(selected, |this| this.bg(gpui_kit::rgba(0x5aa9ff1f)))
                    .when(!selected, |this| {
                        this.hover(|style| style.bg(ui::surface_hover()))
                    })
                    .when(is_dir, |this| {
                        this.child(icon(
                            if folded {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            },
                            12.0,
                            text_muted(),
                        ))
                        .child(icon(
                            if folded {
                                IconName::Folder
                            } else {
                                IconName::FolderOpen
                            },
                            14.0,
                            text_muted(),
                        ))
                    })
                    .child(ui::ellipsis(file_name(item.path())))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        cx.stop_propagation();
                        if is_dir {
                            if !searching {
                                this.toggle_import_dir(index, cx);
                            }
                        } else {
                            this.select_import_item(index, cx);
                        }
                    }))
                    .into_any_element()
            }));

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
            .child(tabs)
            .child(
                div().w_full().child(
                    Input::new(&dialog.search)
                        .id("playlist-import-search")
                        .w_full(),
                ),
            );

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
        } else if no_match {
            body = body.child(
                div()
                    .py(px(24.0))
                    .flex()
                    .justify_center()
                    .text_size(px(12.0))
                    .text_color(text_faint())
                    .child("没有匹配的文件"),
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
            self.remember_history(format!("导入 {imported} 个素材"));
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

/// 一个素材该落到哪些轨道：有画面进第一条可用视频轨、有声音进第一条可用音频轨。
///
/// 多轨叠放是给后期手动摆素材用的，导入不该把同一段复制进每一条同类轨：
/// 那样加一条视频轨就多一份画面，导出时还以为自己接了多机位。锁定的轨道
/// 仍然跳过，从剩下的里面挑第一条。
fn target_tracks(
    sequence: &crate::timeline::Sequence,
    has_video: bool,
    has_audio: bool,
) -> Vec<crate::timeline::Id> {
    let mut targets = Vec::new();
    if has_video && let Some(track) = first_unlocked(sequence, TrackKind::Video) {
        targets.push(track);
    }
    if has_audio && let Some(track) = first_unlocked(sequence, TrackKind::Audio) {
        targets.push(track);
    }
    targets
}

fn first_unlocked(
    sequence: &crate::timeline::Sequence,
    kind: TrackKind,
) -> Option<crate::timeline::Id> {
    sequence
        .tracks
        .iter()
        .find(|track| track.kind == kind && !track.locked)
        .map(|track| track.id)
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
    fn 多条同类轨道只进第一条() {
        let mut sequence = Sequence::new("测试", Rational::FPS_25, 1920, 1080);
        let video = sequence.tracks[0].id;
        let audio = sequence.tracks[1].id;
        sequence.add_track(crate::timeline::TrackKind::Video);
        sequence.add_track(crate::timeline::TrackKind::Audio);

        assert_eq!(target_tracks(&sequence, true, true), vec![video, audio]);
    }

    #[test]
    fn 锁定的轨道不收素材() {
        let mut sequence = Sequence::new("测试", Rational::FPS_25, 1920, 1080);
        let video = sequence.tracks[0].id;
        sequence.track_mut(video).unwrap().locked = true;
        assert!(target_tracks(&sequence, true, false).is_empty());

        // 第一条锁了就顺延到第二条，而不是整批导入失败。
        let second = sequence.add_track(crate::timeline::TrackKind::Video);
        assert_eq!(target_tracks(&sequence, true, false), vec![second]);
    }
}
