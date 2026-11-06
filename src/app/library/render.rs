//! 面板绘制：标题、导入按钮、搜索框、素材行与底部。

use super::super::*;
use super::covers::spec;
use super::panel::ThumbState;
use crate::timeline::{Asset, AssetId, AssetState, MediaInfo};
use gpui_kit::component::input::Input;
use std::path::PathBuf;

/// 一帧最多投递几个缩略图请求：滚动列表时请求一条条补，不会一次把队列塞满。
const THUMB_BATCH: usize = 4;

impl PlayerApp {
    /// 左侧素材库。`bottom` 是时间线高度：面板只占预览区，不压住时间线。
    pub(crate) fn render_library(
        &mut self,
        top: Pixels,
        width: f32,
        bottom: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if width <= 0.0 || self.project.is_none() || self.library.is_none() {
            return None;
        }
        self.request_missing_thumbs();

        let project = self.project.as_ref()?;
        let library = self.library.as_ref()?;
        let query = library.query.trim().to_lowercase();
        let rows: Vec<&Asset> = project
            .assets
            .iter()
            .filter(|asset| {
                query.is_empty() || asset.display_name().to_lowercase().contains(&query)
            })
            .collect();
        let total = project.assets.len();

        let mut list = div()
            .id("library-list")
            .flex()
            .flex_col()
            .gap(px(2.0))
            .px(px(6.0))
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll();
        for asset in &rows {
            list = list.child(self.render_library_row(asset, cx));
        }
        if rows.is_empty() {
            let empty = if total == 0 {
                "还没有素材。先「导入文件」或「导入文件夹」。"
            } else {
                "没有匹配的素材"
            };
            list = list.child(
                div()
                    .py(px(24.0))
                    .flex()
                    .justify_center()
                    .text_size(px(11.0))
                    .text_color(text_faint())
                    .whitespace_normal()
                    .child(empty),
            );
        }

        let selected = library.selected;
        let can_remove = selected.is_some();
        let importing = self
            .library_import
            .as_ref()
            .map(|import| format!("正在探测素材 {}/{}", import.done, import.total));

        let mut panel = div()
            .id("library-panel")
            .absolute()
            .top(top)
            .left_0()
            .bottom(px(bottom))
            .w(px(width))
            .flex()
            .flex_col()
            .bg(gpui_kit::rgba(0x131317e6))
            .border_r_1()
            .border_color(gpui_kit::rgb(0x26262e))
            // 面板上的点击不该漏到画面去（画面那一层负责播放 / 暂停）。
            .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                cx.stop_propagation();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pl(px(14.0))
                    .pr(px(6.0))
                    .py(px(8.0))
                    .child(
                        div()
                            .text_size(px(14.0))
                            .text_color(text())
                            .child(format!("素材库 · {total}")),
                    )
                    .child(icon_button(
                        "library-close",
                        IconName::X,
                        false,
                        false,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.close_library(cx);
                        }),
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(10.0))
                    .pb(px(8.0))
                    .child(settings_button(
                        "library-import-files",
                        "导入文件…",
                        true,
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.pick_assets(window, cx);
                        }),
                    ))
                    .child(settings_button(
                        "library-import-folder",
                        "导入文件夹…",
                        true,
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.pick_asset_folder(window, cx);
                        }),
                    )),
            )
            .child(
                div()
                    .px(px(10.0))
                    .pb(px(8.0))
                    .w_full()
                    .child(Input::new(&library.search).id("library-search").w_full()),
            );

        if let Some(note) = importing {
            panel = panel.child(
                div()
                    .px(px(14.0))
                    .pb(px(6.0))
                    .text_size(px(11.0))
                    .text_color(ui::accent())
                    .child(note),
            );
        }
        if let Some(audition) = self.audition {
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(6.0))
                    .mx(px(10.0))
                    .mb(px(6.0))
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .bg(gpui_kit::rgba(0x5aa9ff1f))
                    .text_size(px(11.0))
                    .text_color(ui::accent())
                    .child(ui::ellipsis(
                        project
                            .asset(audition)
                            .map(Asset::display_name)
                            .unwrap_or_else(|| "原片".to_string()),
                    ))
                    .child(settings_button(
                        "library-back-timeline",
                        "回到时间线",
                        true,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.stop_audition(cx);
                        }),
                    )),
            );
        }

        panel
            .child(list)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .p(px(10.0))
                    .border_t_1()
                    .border_color(gpui_kit::rgb(0x26262e))
                    .child(settings_button(
                        "library-remove",
                        "移除所选",
                        can_remove,
                        cx.listener(|this, _: &ClickEvent, _window, cx| {
                            this.remove_selected_asset(cx);
                        }),
                    ))
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(text_faint())
                            .whitespace_normal()
                            .child("双击试听原片"),
                    ),
            )
            .into_any_element()
            .into()
    }

    /// 素材库的一行：封面 + 名字 + 时长与规格。
    fn render_library_row(&self, asset: &Asset, cx: &mut Context<Self>) -> AnyElement {
        let Some(library) = self.library.as_ref() else {
            return div().into_any_element();
        };
        let selected = library.selected == Some(asset.id);
        let state = library
            .states
            .get(&asset.id)
            .copied()
            .unwrap_or(AssetState::Ready);
        let cover = match library.thumbs.get(&asset.id) {
            Some(ThumbState::Ready(image)) => Some(image.clone()),
            _ => None,
        };
        let has_cover = cover.is_some();
        let name = asset.display_name();
        let meta = asset
            .media
            .as_ref()
            .map(|media| format!("{} · {}", format_time(media.duration.secs()), spec(media)))
            .unwrap_or_else(|| "尚未探测".to_string());
        // 状态只在真的有问题时才占一行：正常素材上写「可用」只是噪音。
        let note = match state {
            AssetState::Ready => None,
            AssetState::Missing => Some("缺失"),
            AssetState::Changed => Some("文件已改动"),
        };
        let id = asset.id;

        div()
            .id(SharedString::from(format!("library-row-{}", id.raw())))
            .flex()
            .items_center()
            .gap(px(8.0))
            .p(px(4.0))
            .rounded(px(6.0))
            .cursor(CursorStyle::PointingHand)
            .when(selected, |this| this.bg(gpui_kit::rgba(0x5aa9ff1f)))
            .when(!selected, |this| {
                this.hover(|style| style.bg(ui::surface_hover()))
            })
            .child(
                div()
                    .relative()
                    .w(px(72.0))
                    .h(px(40.0))
                    .flex_none()
                    .rounded(px(3.0))
                    .overflow_hidden()
                    .bg(gpui_kit::rgb(0x000000))
                    .child(ui::video_surface(cover))
                    .when(!has_cover, |this| {
                        this.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon(IconName::Film, 14.0, text_faint())),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(if state == AssetState::Missing {
                                text_faint()
                            } else {
                                text()
                            })
                            .child(ui::ellipsis(name)),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(if state == AssetState::Missing {
                                ui::danger()
                            } else {
                                text_faint()
                            })
                            .child(ui::ellipsis(match note {
                                Some(label) => format!("{label} · {meta}"),
                                None => meta,
                            })),
                    ),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
                cx.stop_propagation();
                if event.click_count() >= 2 {
                    this.audition_asset(id, cx);
                } else {
                    this.select_library_asset(id, cx);
                }
            }))
            .into_any_element()
    }

    /// 给还没封面的素材投递缩略图请求。每帧最多几个，滚到哪补到哪。
    fn request_missing_thumbs(&mut self) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let mut wanted: Vec<(AssetId, PathBuf)> = Vec::new();
        {
            let Some(library) = self.library.as_ref() else {
                return;
            };
            for asset in &project.assets {
                if wanted.len() >= THUMB_BATCH {
                    break;
                }
                // 纯音频没有画面可抽，缺文件的也抽不出来：这两种都不必跑 ffmpeg。
                if asset.missing || !asset.media.as_ref().is_some_and(MediaInfo::has_video) {
                    continue;
                }
                if library.thumbs.contains_key(&asset.id) || library.requested.contains(&asset.id) {
                    continue;
                }
                wanted.push((asset.id, asset.resolved.clone()));
            }
        }
        let Some(library) = self.library.as_mut() else {
            return;
        };
        for (id, path) in wanted {
            library.requested.insert(id);
            library.worker.request(id, path);
        }
    }
}
