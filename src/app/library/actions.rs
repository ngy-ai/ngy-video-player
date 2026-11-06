//! 面板的开关、素材状态的刷新、选中 / 移除与试听。

use super::super::*;
use super::panel::{LibraryPanel, ThumbState, ThumbWorker};
use crate::timeline::{AssetId, AssetState};
use gpui_kit::component::input::InputState;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

impl PlayerApp {
    /// 开关素材库面板。
    pub(crate) fn toggle_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.is_none() {
            self.flash("先打开或新建一个工程");
            cx.notify();
            return;
        }
        if self.library.is_some() {
            self.close_library(cx);
            return;
        }
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索素材名"));
        let search_changes = cx.observe(&search, |this, state, cx| {
            let query = state.read(cx).value().to_string();
            let Some(library) = this.library.as_mut() else {
                return;
            };
            // 光标移动也会触发通知，只有文本真的变了才重绘列表。
            if library.query == query {
                return;
            }
            library.query = query;
            cx.notify();
        });
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let worker = ThumbWorker::spawn(tx);
        self.library = Some(LibraryPanel {
            search,
            query: String::new(),
            selected: BTreeSet::new(),
            anchor: None,
            thumbs: HashMap::new(),
            requested: HashSet::new(),
            states: HashMap::new(),
            worker,
            _search_changes: search_changes,
        });
        self.refresh_library_states();
        // 缩略图回来时写回面板。循环在面板关掉后靠 `update` 失败退出。
        cx.spawn(async move |this, cx| {
            while let Some((id, image)) = rx.next().await {
                let alive = this.update(cx, |this, cx| {
                    let Some(library) = this.library.as_mut() else {
                        return;
                    };
                    library.requested.remove(&id);
                    if let Some(image) = image {
                        library.thumbs.insert(id, ThumbState::Ready(image));
                    } else {
                        library.thumbs.insert(id, ThumbState::Failed);
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        self.timeline_menu = None;
        self.menu_bar = None;
        cx.notify();
    }

    /// 换工程：面板、导入进度、试听都属于上一个工程，一起丢掉。
    ///
    /// 正在跑的探测靠代数作废：它还会把结果发回来，但写不进新工程的素材表。
    pub(crate) fn reset_library(&mut self) {
        self.library = None;
        self.library_import = None;
        self.library_generation = self.library_generation.wrapping_add(1);
        self.audition = None;
        self.clear_source();
    }

    pub(crate) fn close_library(&mut self, cx: &mut Context<Self>) {
        if self.library.take().is_none() {
            return;
        }
        // 面板关了就没地方「回到时间线」，试听顺手收掉。
        if self.audition.take().is_some() {
            self.timeline_preview = None;
            self.preview_at_playhead(cx);
        }
        // 三点编辑的源区间也跟着面板走：面板都没了，按 `,` / `.` 也无从确认取的是哪一截。
        self.clear_source();
        cx.notify();
    }

    /// 面板占的宽度。全屏与没有工程时不占地方。
    pub(crate) fn library_width(&self, window: &Window) -> f32 {
        if window.is_fullscreen() || self.project.is_none() || self.library.is_none() {
            return 0.0;
        }
        let available = f32::from(window.viewport_size().width);
        LIBRARY_PANEL_WIDTH.min((available - 240.0).max(140.0))
    }

    /// 重新统计每个素材的状态。`fs::metadata` 只在这里跑。
    pub(crate) fn refresh_library_states(&mut self) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let states: HashMap<AssetId, AssetState> = project
            .assets
            .iter()
            .map(|asset| (asset.id, asset.state()))
            .collect();
        let Some(library) = self.library.as_mut() else {
            return;
        };
        // 文件被换过的素材，旧封面已经不是那部片子了，清掉重新生成。
        for (id, state) in &states {
            if *state == AssetState::Changed {
                library.thumbs.remove(id);
            }
        }
        library.states = states;
    }

    /// 点素材行。与播放列表同一套多选：普通点击单选，Ctrl/⌘ 切换，Shift 连选一段。
    pub(crate) fn select_library_asset(
        &mut self,
        id: AssetId,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        // 顺序按素材表来：拖出去的时候按列表里看到的顺序首尾相接，而不是按 id 排。
        let order: Vec<AssetId> = project.assets.iter().map(|asset| asset.id).collect();
        let Some(library) = self.library.as_mut() else {
            return;
        };
        if modifiers.shift {
            let anchor = library.anchor.unwrap_or(id);
            let from = order.iter().position(|item| *item == anchor);
            let to = order.iter().position(|item| *item == id);
            if let (Some(from), Some(to)) = (from, to) {
                let (start, end) = if from <= to { (from, to) } else { (to, from) };
                // 连选替换上一次的区间，否则反复 Shift 点击只会越选越多。
                library.selected = order[start..=end].iter().copied().collect();
            }
            library.anchor.get_or_insert(id);
        } else if modifiers.secondary(cx) {
            if !library.selected.remove(&id) {
                library.selected.insert(id);
            }
            library.anchor = Some(id);
        } else {
            library.selected.clear();
            library.selected.insert(id);
            library.anchor = Some(id);
        }
        cx.notify();
    }

    /// 选中的素材，按它们在本工程素材表里的顺序排好。
    pub(crate) fn selected_assets(&self) -> Vec<crate::timeline::Asset> {
        let Some(project) = self.project.as_ref() else {
            return Vec::new();
        };
        let Some(library) = self.library.as_ref() else {
            return Vec::new();
        };
        project
            .assets
            .iter()
            .filter(|asset| library.selected.contains(&asset.id))
            .cloned()
            .collect()
    }

    /// 把选中的素材从素材库里移除。还被片段引用时拒绝并说明原因。
    pub(crate) fn remove_selected_asset(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<AssetId> = self
            .library
            .as_ref()
            .map(|library| library.selected.iter().copied().collect())
            .unwrap_or_default();
        if ids.is_empty() {
            return;
        }
        // 整组先查一遍：只要有一个还被片段引用就整组不动，不留「删了一半」的状态。
        let blocked = {
            let Some(project) = self.project.as_mut() else {
                return;
            };
            let blocked = ids.iter().find_map(|id| {
                let clips = project.asset_clip_count(*id);
                if clips == 0 {
                    return None;
                }
                let name = project
                    .asset(*id)
                    .map(|asset| asset.display_name())
                    .unwrap_or_default();
                Some((name, clips))
            });
            if blocked.is_none() {
                for id in &ids {
                    if let Err(error) = project.remove_asset(*id) {
                        log_warn!("移除素材失败：{error}");
                    }
                }
            }
            blocked
        };

        if let Some((name, clips)) = blocked {
            self.error = Some(Banner::new(
                "这个素材还在时间线上",
                format!(
                    "「{name}」还有 {clips} 个片段在用。先把它们从时间线上删掉，再从素材库移除。\
                     \n\n移除只改素材库，不会删掉磁盘上的文件。"
                ),
            ));
            cx.notify();
            return;
        }

        if let Some(library) = self.library.as_mut() {
            library.selected.clear();
            library.anchor = None;
            for id in &ids {
                library.thumbs.remove(id);
                library.requested.remove(id);
                library.states.remove(id);
            }
        }
        self.flash(if ids.len() == 1 {
            "已从素材库移除（磁盘上的文件没删）".to_string()
        } else {
            format!("已移除 {} 个素材（磁盘上的文件没删）", ids.len())
        });
        cx.notify();
    }

    /// 双击素材：在预览区放原片，时间线与播放头一概不动。
    ///
    /// 试听不是一种模式——拖一下播放头、按一下空格都回到时间线预览（见
    /// [`PlayerApp::preview_at_playhead`] 与 [`PlayerApp::toggle_play`]）。
    pub(crate) fn audition_asset(&mut self, id: AssetId, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let Some(asset) = project.asset(id) else {
            return;
        };
        if asset.missing {
            self.error = Some(Banner::new(
                "素材文件不在了",
                format!(
                    "找不到「{}」。把它放回原位再双击，或者重新导入一次。",
                    asset.path.display()
                ),
            ));
            cx.notify();
            return;
        }
        let Some(path) = project.asset_path(id).map(Path::to_path_buf) else {
            return;
        };
        let name = asset.display_name();

        self.stop_timeline_playback(cx);
        // 时间线预览记的是「mpv 里现在是哪个素材」，试听把它顶掉了，要一起清。
        self.timeline_preview = None;
        self.timeline_loaded_at = Some(Instant::now());
        // 换素材就是换一截要取的片段，上一段素材的源入出点留着只会误放。
        self.clear_source();
        self.source_asset = Some(id);
        self.audition = Some(id);
        self.begin_load(&path);
        self.send(Command::Load {
            url: path.to_string_lossy().into_owned(),
            start: None,
        });
        self.send(Command::SetPaused(false));
        self.flash(format!("试听原片：{name}"));
        self.poke();
        cx.notify();
    }

    /// 退出试听、回到播放头预览。源入出点**不**跟着作废。
    ///
    /// 三点编辑里「回到时间线看结果」和「不要这一截了」是两件事：前者随时会发生
    /// （按钮、Esc、点一下时间线），后者得用户明说（面板上的「取消源区间」）。
    /// 混在一起的话，点一下「回到时间线」刚设好的源区间就没了，再按 `,` 还提示去双击素材。
    pub(crate) fn stop_audition(&mut self, cx: &mut Context<Self>) {
        if self.audition.take().is_none() {
            return;
        }
        self.timeline_preview = None;
        self.preview_at_playhead(cx);
        if self.source_asset.is_some() {
            self.flash("已回到时间线预览；源区间还在，按 `,` 插入、`.` 覆盖");
        } else {
            self.flash("已回到时间线预览");
        }
    }
}
