//! 素材标签与智能合集（M10-T08）。
//!
//! 标签是剪片时才有的判断（「航拍」「空镜」「废片」），文件里没有，所以存在工程里。
//! 批量打标签只动 `project.assets`，不进撤销栈：它不是剪辑动作，撤销一次剪辑把标签
//! 一起退回去反而奇怪。合集存的是条件串，点一下把它填进搜索框——搜索框是唯一入口，
//! 少一套「合集内再搜」的状态就不会有第一层与第二层条件打架的问题。

use super::super::*;
use crate::timeline::query::Collection;

impl PlayerApp {
    /// 给选中的素材加 / 去一个标签。`tag` 为空时什么都不做（按钮那边也会置灰）。
    pub(crate) fn tag_selected(
        &mut self,
        tag: &str,
        add: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tag = tag.trim();
        if tag.is_empty() {
            self.flash("先在上面的框里写一个标签");
            cx.notify();
            return;
        }
        let Some(library) = self.library.as_ref() else {
            return;
        };
        let selected: Vec<_> = library.selected.iter().copied().collect();
        if selected.is_empty() {
            self.flash("先在列表里选中素材");
            cx.notify();
            return;
        }
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let mut touched = 0;
        for id in &selected {
            let Some(asset) = project.assets.iter_mut().find(|asset| asset.id == *id) else {
                continue;
            };
            if add {
                // 同一个标签不重复加：重复只会让合集条件看起来生效了其实没加。
                if !asset.tags.iter().any(|existing| existing == tag) {
                    asset.tags.push(tag.to_string());
                    touched += 1;
                }
            } else {
                let before = asset.tags.len();
                asset.tags.retain(|existing| existing != tag);
                if asset.tags.len() != before {
                    touched += 1;
                }
            }
        }
        self.flash(if add {
            format!("给 {touched} 个素材加了标签「{tag}」")
        } else {
            format!("从 {touched} 个素材去掉了标签「{tag}」")
        });
        self.note_edit();
        cx.notify();
    }

    /// 合集条：点一下把条件填进搜索框；「存为合集」把当前条件存下来。
    pub(crate) fn render_library_collections(
        &self,
        project: &crate::timeline::Project,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = self
            .library
            .as_ref()
            .map(|library| library.query.clone())
            .unwrap_or_default();
        let mut row = div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .px(px(10.0))
            .pb(px(6.0))
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(text_faint())
                    .child("合集"),
            );
        for (index, collection) in project.collections.iter().enumerate() {
            let active = collection.query.trim() == current.trim();
            let query = collection.query.clone();
            // 名字是用户给的，`chip` 只收 `&'static str`，所以这里自己摆相同的样式。
            row = row.child(
                div()
                    .id(("library-collection", index))
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(5.0))
                    .flex_none()
                    .bg(if active { ui::accent() } else { ui::track() })
                    .text_size(px(10.0))
                    .text_color(if active { bg() } else { text_muted() })
                    .cursor(CursorStyle::PointingHand)
                    .child(collection.name.clone())
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.apply_collection(&query, window, cx);
                    })),
            );
        }
        let can_save = !current.trim().is_empty()
            && !project
                .collections
                .iter()
                .any(|collection| collection.query.trim() == current.trim());
        row = row.child(settings_button(
            "library-collection-save",
            "存为合集",
            can_save,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.save_collection(cx);
            }),
        ));
        row.into_any_element()
    }

    /// 点合集：把条件串填进搜索框（搜索框是唯一入口）。
    fn apply_collection(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(library) = self.library.as_ref() else {
            return;
        };
        let input = library.search.clone();
        if let Some(library) = self.library.as_mut() {
            library.query = query.to_string();
        }
        input.update(cx, |state, cx| {
            state.set_value(query.to_string(), window, cx);
        });
        cx.notify();
    }

    /// 把当前条件存成合集。名字先用条件串本身：多数人一眼就能认出来，
    /// 想改名直接改条件再存一条也不费事。
    fn save_collection(&mut self, cx: &mut Context<Self>) {
        let Some(library) = self.library.as_ref() else {
            return;
        };
        let query = library.query.trim().to_string();
        if query.is_empty() {
            return;
        }
        let Some(project) = self.project.as_mut() else {
            return;
        };
        project
            .collections
            .push(Collection::new(query.clone(), query.clone()));
        self.flash(format!("已存为合集：{query}"));
        self.note_edit();
        cx.notify();
    }
}
