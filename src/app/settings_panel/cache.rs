//! 设置窗口 ·「渲染缓存」页（M10-T19）：开关、容量上限、补缓存队列与清理。
//!
//! 为什么单独一页：这一页的每一项都会动到磁盘与 CPU，跟「画面怎么摆」「谁来回放」
//! 不是一类偏好。放在这里也能顺便把「现在占了多少、还差哪几段没渲」说清楚。

use super::super::render_cache_queue::{CACHE_LIMIT_CHOICES, cache_progress_label};
use super::super::*;

impl PlayerApp {
    pub(super) fn cache_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let enabled_row = picture_option(
            "settings-render-cache",
            "剪辑预览使用渲染缓存",
            self.settings.render_cache,
            true,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.settings.render_cache = !this.settings.render_cache;
                this.settings.save();
                // 开关一变，预览该换文件：让下一次入场重新走 `preview_at_playhead`。
                this.preview_at_playhead(cx);
                cx.notify();
            }),
        );

        let limit_rows: Vec<AnyElement> = CACHE_LIMIT_CHOICES
            .into_iter()
            .enumerate()
            .map(|(index, megabytes)| {
                let selected = self.settings.render_cache_limit_mb == megabytes;
                engine_option(
                    format!("settings-cache-limit-{index}"),
                    crate::timeline::size_label(megabytes * 1024 * 1024),
                    None,
                    selected,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.settings.render_cache_limit_mb = megabytes;
                        this.settings.save();
                        // 上限可能被调小了：立刻按新上限收一遍，别等下次补缓存。
                        let removed = this.trim_render_cache();
                        if removed > 0 {
                            this.flash(format!("按新上限删掉 {removed} 个旧缓存"));
                        }
                        cx.notify();
                    }),
                )
            })
            .collect();

        let missing = self.missing_cache_count();
        let busy = self.cache_queue.is_some();
        // 按钮标题用静态文案（`settings_button` 只收 `&'static str`），
        // 缺几段这件事写在下面那行说明里。
        let fill_state = if busy {
            "正在补缓存…".to_string()
        } else if missing == 0 {
            "当前序列没有要补的片段".to_string()
        } else {
            format!("还有 {missing} 段没缓存")
        };

        let actions = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(settings_button(
                "settings-cache-fill",
                "为时间线补缓存",
                !busy && missing > 0,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    let queued = this.queue_missing_caches(cx);
                    if queued == 0 {
                        this.flash("没有要补的片段");
                    }
                    cx.notify();
                }),
            ))
            .child(settings_button(
                "settings-cache-clear",
                "清理缓存",
                self.cache_bytes > 0 && !busy,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.clear_render_cache(cx);
                }),
            ))
            .when(busy, |this| {
                this.child(settings_button(
                    "settings-cache-cancel",
                    "取消",
                    true,
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.cancel_cache_queue(cx);
                    }),
                ))
            })
            .into_any_element();

        let usage = format!(
            "当前占用 {} / 上限 {} · {fill_state}",
            crate::timeline::size_label(self.cache_bytes),
            self.cache_limit_label()
        );

        let mut rows = vec![enabled_row];
        if let Some(queue) = self.cache_queue.as_ref() {
            rows.push(settings_note(cache_progress_label(queue)));
        }

        div()
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(option_group("预览", rows))
            .child(option_group("容量上限", limit_rows))
            .child(option_group("缓存", vec![actions, settings_note(usage)]))
            .child(settings_note(
                "缓存只给预览用：导出永远重新算一遍，不受这里影响。有启用效果、\
                 调过色或打了会改画面的关键帧的片段才会进缓存；倍率不是 1x 或带片尾\
                 定格的片段不缓存（缓存文件的时间轴对不上）。超出上限时按最旧的先删。",
            ))
            .child(settings_note(format!(
                "目录：{}",
                self.render_cache_dir().display()
            )))
            .into_any_element()
    }
}
