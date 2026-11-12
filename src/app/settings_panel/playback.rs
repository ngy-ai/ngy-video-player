//! 设置窗口 ·「播放」页：循环模式与置顶策略。
//!
//! 这两项都是「下次打开播放器也该照这个来」的偏好，控制条和「视图」菜单里也有快捷
//! 入口；设置窗口这一份是为了让「所有会记住的选择」有一个能一次看完的地方。

use super::super::*;

impl PlayerApp {
    pub(super) fn playback_page(
        &self,
        on_top_supported: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let loop_rows: Vec<AnyElement> = [LoopMode::Off, LoopMode::All, LoopMode::One]
            .into_iter()
            .enumerate()
            .map(|(index, mode)| {
                engine_option(
                    format!("settings-loop-{index}"),
                    mode.label().to_string(),
                    None,
                    self.settings.loop_mode == mode,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.set_loop(mode, cx);
                    }),
                )
            })
            .collect();

        // 置顶要拿原生窗口句柄：Wayland 没给普通窗口留这个能力，只能换成一句说明。
        let on_top_rows: Vec<AnyElement> = if on_top_supported {
            OnTop::ALL
                .into_iter()
                .enumerate()
                .map(|(index, mode)| {
                    engine_option(
                        format!("settings-on-top-{index}"),
                        mode.label().to_string(),
                        None,
                        self.settings.on_top == mode,
                        cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            this.set_on_top(mode);
                            cx.notify();
                        }),
                    )
                })
                .collect()
        } else {
            vec![settings_note("当前窗口系统不支持置顶")]
        };

        // 剪辑预览用不用代理：只影响预览，导出永远走原片，所以放在这里当性能开关。
        let proxy_row = picture_option(
            "settings-preview-proxy",
            "剪辑预览自动使用代理",
            self.settings.preview_proxy,
            true,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.toggle_preview_proxy();
                cx.notify();
            }),
        );

        div()
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(option_group("循环模式", loop_rows))
            .child(option_group("窗口置顶", on_top_rows))
            .child(option_group("剪辑预览", vec![proxy_row]))
            .child(settings_note("两项都跟着偏好保存，下次启动照这个状态来。"))
            .child(settings_note(
                "素材比序列大、旁边又有 `<原名>.proxy.mp4` 时，预览改放代理；导出不受影响。",
            ))
            .into_any_element()
    }
}
