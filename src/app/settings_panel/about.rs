//! 设置窗口 ·「关于」页：版本、技术栈与运行环境。
//!
//! 菜单栏「帮助 → 关于」直接停在这一页。放在模态窗口里而不是弹一条提示条：
//! 版本号、解码引擎有没有装上、配置和日志落在哪儿，都是出问题时要回头看一眼的
//! 信息，一闪而过的提示读不完，也没法对着抄路径。

use super::super::*;

impl PlayerApp {
    pub(super) fn about_page(&self) -> AnyElement {
        // 引擎缺失时程序仍能启动，只是放不了片；这一页正好是用户来确认原因的地方。
        let engine = if self.engine.is_some() {
            "libmpv 已加载".to_string()
        } else {
            "未找到 libmpv，暂时无法播放".to_string()
        };

        div()
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(about_hero())
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(text_muted())
                    .child(env!("CARGO_PKG_DESCRIPTION")),
            )
            .child(option_group(
                "运行环境",
                vec![
                    info_row("解码引擎", engine),
                    info_row(
                        "支持格式",
                        format!("{} 种常见容器与编码", formats::supported_extension_count()),
                    ),
                    info_row("配置与日志", local_data_dir()),
                ],
            ))
            .child(option_group(
                "技术栈",
                vec![
                    info_row("界面", "GPUI（Zed）· 全部界面为纯 Rust 自绘".to_string()),
                    info_row("解码", "libmpv · 解复用、解码、字幕都由它负责".to_string()),
                ],
            ))
            .child(settings_note(
                "设置与日志跟着系统惯例存放；NGY_VIDEO_CONFIG_DIR、NGY_VIDEO_LOG_DIR 可以改位置。",
            ))
            .child(settings_note(
                "GPUI 与 libmpv 各自遵循其许可；libmpv 在运行时动态加载，不随本程序分发。",
            ))
            .into_any_element()
    }
}

/// 名字 + 版本，看得见的第一眼就该知道「这是哪个程序、哪个版本」。
fn about_hero() -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(14.0))
        .child(
            div()
                .size(px(46.0))
                .flex_none()
                .rounded(px(12.0))
                .bg(ui::surface_hover())
                .flex()
                .items_center()
                .justify_center()
                .child(icon(IconName::Clapperboard, 24.0, ui::accent())),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(17.0))
                        .text_color(text())
                        .child("ngy-video-player"),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(text_faint())
                        .child(format!(
                            "版本 {} · {} 许可",
                            env!("CARGO_PKG_VERSION"),
                            env!("CARGO_PKG_LICENSE")
                        )),
                ),
        )
        .into_any_element()
}

/// 一行「左标题 + 右内容」。内容列吃满剩余宽度；长路径从中间省略，
/// 盘符和最后一级目录都还看得见，比只留开头有用。
fn info_row(key: &'static str, value: String) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(12.0))
        .h(px(26.0))
        .child(
            div()
                .w(px(76.0))
                .flex_none()
                .text_size(px(12.0))
                .text_color(text_muted())
                .child(key),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis_middle()
                .text_size(px(13.0))
                .text_color(text())
                .child(value),
        )
        .into_any_element()
}

/// 配置和日志所在的目录。两者通常同一个，取不到配置路径时退到日志路径。
fn local_data_dir() -> String {
    let from_config =
        crate::settings::config_path().and_then(|path| path.parent().map(|dir| dir.to_path_buf()));
    let from_log =
        crate::logging::log_path().and_then(|path| path.parent().map(|dir| dir.to_path_buf()));
    from_config
        .or(from_log)
        .map(|dir| dir.display().to_string())
        .unwrap_or_else(|| "不可用".to_string())
}
