//! 侧边设置面板：画面、引擎、快捷键帮助。

use super::*;

impl PlayerApp {
    /// 「画面设置」面板：比例 / 旋转 / 镜像三组。
    ///
    /// 用居中模态而不是贴在控制条上方的下拉：三组加起来十几项，挤在控制条上方
    /// 会把画面压掉一截；而这几项都是「调一次就不动」的，不需要随手可达。
    pub(super) fn render_picture(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.show_picture {
            return None;
        }

        let aspect = self.settings.video_aspect;
        let rotation = self.settings.video_rotate;

        let aspect_rows: Vec<AnyElement> = VideoAspect::ALL
            .chunks(2)
            .enumerate()
            .map(|(row, pair)| {
                div()
                    .flex()
                    .gap(px(4.0))
                    .children(pair.iter().enumerate().map(|(column, mode)| {
                        picture_option(
                            format!("picture-aspect-{row}-{column}"),
                            mode.label(),
                            aspect == *mode,
                            false,
                            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                this.set_video_aspect(*mode);
                                cx.notify();
                            }),
                        )
                    }))
                    .into_any_element()
            })
            .collect();

        let rotation_rows: Vec<AnyElement> = Rotation::ALL
            .chunks(2)
            .enumerate()
            .map(|(row, pair)| {
                div()
                    .flex()
                    .gap(px(4.0))
                    .children(pair.iter().enumerate().map(|(column, angle)| {
                        picture_option(
                            format!("picture-rotate-{row}-{column}"),
                            angle.label(),
                            rotation == *angle,
                            false,
                            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                this.set_rotation(*angle);
                                cx.notify();
                            }),
                        )
                    }))
                    .into_any_element()
            })
            .collect();

        let mirror_row = div()
            .flex()
            .gap(px(4.0))
            .child(picture_option(
                "picture-mirror-h",
                "水平镜像",
                self.settings.mirror_h,
                true,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.toggle_mirror(true);
                    cx.notify();
                }),
            ))
            .child(picture_option(
                "picture-mirror-v",
                "垂直镜像",
                self.settings.mirror_v,
                true,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.toggle_mirror(false);
                    cx.notify();
                }),
            ))
            .into_any_element();

        Some(
            div()
                .id("picture-settings")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui_kit::rgba(0x000000b8))
                // 只拦住鼠标按下，让同位置的视频画面收不到这次点击。
                //
                // 这里刻意不用 `on_click` 去关面板：GPUI 里祖先元素的 click 监听
                // 会先于里面的选项执行，一旦在遮罩层上关面板，点选项就再也不生效了。
                // 收起面板交给右上角的 X 和 Esc，和「查看格式」面板保持一致。
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_this, _: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                    }),
                )
                .child(
                    div()
                        .id("picture-settings-card")
                        .w(px(520.0))
                        .p(px(20.0))
                        .rounded(px(14.0))
                        .bg(surface())
                        .border_1()
                        .border_color(gpui_kit::rgb(0x2c2c34))
                        .flex()
                        .flex_col()
                        .gap(px(16.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_this, _: &MouseDownEvent, _window, cx| {
                                cx.stop_propagation();
                            }),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(px(15.0))
                                        .text_color(text())
                                        .child("画面设置"),
                                )
                                .child(icon_button(
                                    "picture-close",
                                    IconName::X,
                                    false,
                                    false,
                                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                                        this.set_picture_visible(false);
                                        cx.notify();
                                    }),
                                )),
                        )
                        .child(option_group("画面比例", aspect_rows))
                        .child(option_group("画面旋转", rotation_rows))
                        .child(option_group("画面镜像", vec![mirror_row]))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(text_faint())
                                .child("这几项跟着偏好保存，换片和重启都保持"),
                        ),
                ),
        )
    }

    /// 「视频渲染与解码」面板：渲染模式 / 解码模式 / 播放显卡三组。
    ///
    /// 和「画面设置」分成两个面板：那边调的是「同一帧怎么摆」（比例、旋转、镜像），
    /// 这里换的是「谁来画、谁来解码」。后者更像排障开关，混在一起只会把常用项挤远。
    pub(super) fn render_engine(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.show_engine {
            return None;
        }

        let render_rows: Vec<AnyElement> = RenderMode::ALL
            .into_iter()
            .enumerate()
            .map(|(index, mode)| {
                engine_option(
                    format!("engine-render-{index}"),
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
                    format!("engine-decode-{index}"),
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
                "engine-gpu-auto",
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
                    format!("engine-gpu-{index}"),
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

        Some(
            div()
                .id("engine-settings")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui_kit::rgba(0x000000b8))
                // 只拦住鼠标按下，让同位置的视频画面收不到这次点击。
                // 同样不用 `on_click` 关面板：祖先的 click 会先于里面的选项执行。
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_this, _: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                    }),
                )
                .child(
                    div()
                        .id("engine-settings-card")
                        .w(px(520.0))
                        // 显卡多的机器（工作站/笔记本双卡）也不会把面板顶出窗口。
                        .max_h(px(560.0))
                        .overflow_y_scroll()
                        .p(px(20.0))
                        .rounded(px(14.0))
                        .bg(surface())
                        .border_1()
                        .border_color(gpui_kit::rgb(0x2c2c34))
                        .flex()
                        .flex_col()
                        .gap(px(16.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_this, _: &MouseDownEvent, _window, cx| {
                                cx.stop_propagation();
                            }),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(px(15.0))
                                        .text_color(text())
                                        .child("视频渲染与解码"),
                                )
                                .child(icon_button(
                                    "engine-close",
                                    IconName::X,
                                    false,
                                    false,
                                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                                        this.set_engine_visible(false);
                                        cx.notify();
                                    }),
                                )),
                        )
                        .child(option_group("视频渲染模式", render_rows))
                        .child(option_group("视频解码模式", decode_rows))
                        .child(option_group("播放显卡设置", gpu_rows))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .text_size(px(11.0))
                                .text_color(text_faint())
                                .child("这三项跟着偏好保存，换片和重启都保持")
                                .child(
                                    "解码方式与显卡在重新打开文件后一定生效；软件解码时与显卡无关",
                                ),
                        ),
                ),
        )
    }

    pub(super) fn render_help(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.show_help {
            return None;
        }
        let rows = [
            ("空格", "播放 / 暂停"),
            ("← →", "快退 / 快进 5 秒"),
            ("↑ ↓", "音量"),
            ("滚轮", "音量"),
            ("Ctrl/⌘ 滚轮", "逐帧前进 / 后退"),
            ("N / P", "下一个 / 上一个"),
            ("[ ]", "减速 / 加速"),
            ("L", "播放列表"),
            ("Ctrl/⌘ 点击", "列表多选"),
            ("Shift 点击", "连选一段"),
            ("右键列表项", "删除 / 定位文件 / 查看格式"),
            ("Delete", "删除所选项"),
            ("M", "静音"),
            ("S", "截图"),
            ("O", "打开文件"),
            ("F / 双击", "全屏"),
            ("T", "小窗"),
            ("A", "画面设置"),
            ("Esc", "退出全屏 / 关闭浮层"),
            ("?", "显示 / 隐藏本帮助"),
        ];
        Some(
            div()
                .id("help")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui_kit::rgba(0x000000b8))
                // 帮助是模态的：点它不该穿透去暂停视频。
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .child(
                    div()
                        .w(px(420.0))
                        .p(px(20.0))
                        .rounded(px(14.0))
                        .bg(surface())
                        .border_1()
                        .border_color(gpui_kit::rgb(0x2c2c34))
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .child(
                            div()
                                .text_size(px(15.0))
                                .text_color(text())
                                .child("键盘快捷键"),
                        )
                        .children(rows.iter().map(|(keys, action)| {
                            div()
                                .flex()
                                .justify_between()
                                .text_size(px(13.0))
                                .child(div().text_color(text_muted()).child(*keys))
                                .child(div().text_color(text()).child(*action))
                        })),
                ),
        )
    }
}
