//! 设置窗口 ·「视频」页：渲染模式、解码方式、播放显卡。
//!
//! 这一页换的是「谁来画、谁来解码」，属于排障/性能开关，所以放在画面之后单独一页。

use super::super::*;

impl PlayerApp {
    pub(super) fn video_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let render_rows: Vec<AnyElement> = RenderMode::ALL
            .into_iter()
            .enumerate()
            .map(|(index, mode)| {
                engine_option(
                    format!("settings-render-{index}"),
                    mode.label().to_string(),
                    Some(mode.detail()),
                    self.settings.render_mode == mode,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.set_render_mode(mode);
                        cx.notify();
                    }),
                )
            })
            .collect();

        let decode_rows: Vec<AnyElement> = DecodeMode::ALL
            .into_iter()
            .enumerate()
            .map(|(index, mode)| {
                engine_option(
                    format!("settings-decode-{index}"),
                    mode.label().to_string(),
                    Some(mode.detail()),
                    self.settings.decode == mode,
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.set_decode_mode(mode);
                        cx.notify();
                    }),
                )
            })
            .collect();

        // 显卡列表可能是空的（非 Windows）。空表时给一句明确说明，而不是留一组空行。
        let gpu_rows: Vec<AnyElement> = if self.gpu_adapters.is_empty() {
            vec![
                div()
                    .px(px(8.0))
                    .py(px(6.0))
                    .text_size(px(11.0))
                    .text_color(text_faint())
                    .child("当前平台不支持列出显卡，将由系统自动选择")
                    .into_any_element(),
            ]
        } else {
            let chosen = self.gpu_selected().map(str::to_string);
            let mut rows = vec![engine_option(
                "settings-gpu-auto",
                format!("智能选择（当前使用：{}）", self.current_gpu_label()),
                Some("由系统按运行环境挑选，通常是第一块显示适配器"),
                chosen.is_none(),
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.set_gpu(None);
                    cx.notify();
                }),
            )];
            for (index, name) in self.gpu_adapters.iter().enumerate() {
                let label = name.clone();
                rows.push(engine_option(
                    format!("settings-gpu-{index}"),
                    label.clone(),
                    None,
                    chosen.as_deref() == Some(name.as_str()),
                    cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        this.set_gpu(Some(label.clone()));
                        cx.notify();
                    }),
                ));
            }
            rows
        };

        div()
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(option_group("视频渲染模式", render_rows))
            .child(option_group("视频解码模式", decode_rows))
            .child(option_group("播放显卡设置", gpu_rows))
            .child(settings_note(
                "这三项跟着偏好保存。解码方式与显卡在重新打开文件后一定生效；软件解码时与显卡无关。",
            ))
            .into_any_element()
    }
}
