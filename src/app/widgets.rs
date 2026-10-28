//! 复用的界面构件：菜单行、分组、占位块、媒体信息卡片。
//!
//! 只依赖传入的数据，不读取 `PlayerApp` 的内部状态。

use super::*;

/// 画面设置里的一行：左边指示器 + 文字，整行可点。
///
/// `checkbox` 只决定指示器是圆是方——单选和复选在交互上没区别（都是点一下生效），
/// 区分形状纯粹是为了让「比例/旋转只能选一个」这件事一眼看得出来。
pub(super) fn picture_option(
    id: impl Into<ElementId>,
    label: &'static str,
    selected: bool,
    checkbox: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let indicator = div()
        .size(px(16.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(if selected { ui::accent() } else { ui::track() })
        .when(checkbox, |this| this.rounded(px(4.0)))
        .when(!checkbox, |this| this.rounded_full())
        .when(selected, |this| {
            this.bg(ui::accent())
                .child(icon(IconName::Check, 11.0, text()))
        });

    div()
        .id(id)
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .h(px(30.0))
        .px(px(8.0))
        .rounded(px(6.0))
        .cursor(CursorStyle::PointingHand)
        .hover(|style| style.bg(ui::surface_hover()))
        .on_click(
            move |event: &ClickEvent, window: &mut Window, cx: &mut App| {
                // 面板底下的视频区域也会收到同位置的点击，不拦住会顺手切换播放/暂停。
                cx.stop_propagation();
                on_click(event, window, cx);
            },
        )
        .child(indicator)
        .child(div().text_size(px(13.0)).text_color(text()).child(label))
        .into_any_element()
}

/// 一组单选设置：小标题 + 若干行。「画面设置」和「视频渲染与解码」共用。
pub(super) fn option_group(title: &'static str, rows: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .pb(px(4.0))
                .text_size(px(12.0))
                .text_color(text_muted())
                .child(title),
        )
        .children(rows)
        .into_any_element()
}

/// 「视频渲染与解码」面板里的一行：单选圆点 + 标题 +（可选）说明。
///
/// 说明文字是给「不知道该选哪档」的人看的，所以固定成两行结构：有说明的选项比
/// 没说明的高一点，同一组里保持一致，切换选项时不会让下面的行上下跳。
pub(super) fn engine_option(
    id: impl Into<ElementId>,
    label: String,
    detail: Option<&'static str>,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let indicator = div()
        .size(px(16.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1()
        .border_color(if selected { ui::accent() } else { ui::track() })
        .when(selected, |this| {
            this.bg(ui::accent())
                .child(icon(IconName::Check, 11.0, text()))
        });

    let mut texts = div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap(px(3.0))
        .child(div().text_size(px(13.0)).text_color(text()).child(label));
    if let Some(detail) = detail {
        texts = texts.child(
            div()
                .text_size(px(11.0))
                .text_color(text_faint())
                .child(detail),
        );
    }

    div()
        .id(id)
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .items_start()
        .gap(px(10.0))
        .px(px(8.0))
        .py(px(6.0))
        .rounded(px(6.0))
        .cursor(CursorStyle::PointingHand)
        .hover(|style| style.bg(ui::surface_hover()))
        .on_click(
            move |event: &ClickEvent, window: &mut Window, cx: &mut App| {
                // 面板底下的视频区域也会收到同位置的点击，不拦住会顺手切换播放/暂停。
                cx.stop_propagation();
                on_click(event, window, cx);
            },
        )
        // 圆点和第一行标题对齐，说明多一行也不会把它拽到中间。
        .child(div().h(px(18.0)).flex().items_center().child(indicator))
        .child(texts)
        .into_any_element()
}

/// 右键菜单里的一行：图标 + 文字，整行可点。
pub(super) fn item_menu_entry(
    id: &'static str,
    icon_name: IconName,
    label: &'static str,
    color: Hsla,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.0))
        .px(px(10.0))
        .py(px(7.0))
        .mx(px(4.0))
        .rounded(px(6.0))
        .text_size(px(13.0))
        .text_color(color)
        .cursor(CursorStyle::PointingHand)
        .hover(|style| style.bg(ui::surface_hover()))
        .child(ui::icon(icon_name, 14.0, color))
        .child(label)
        .on_click(on_click)
        .into_any_element()
}

/// 截图卡片底部的一个动作：图标 + 文字，三等分卡片宽度。
pub(super) fn screenshot_card_action(
    id: &'static str,
    icon_name: IconName,
    label: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex_1()
        .h(px(36.0))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .cursor(CursorStyle::PointingHand)
        .hover(|style| style.bg(ui::surface_hover()))
        .child(ui::icon(icon_name, 14.0, text_muted()))
        .child(div().text_size(px(12.0)).text_color(text()).child(label))
        .on_click(
            move |event: &ClickEvent, window: &mut Window, cx: &mut App| {
                // 卡片浮在画面上，动作不截断的话同一次点击还会漏到视频区暂停播放。
                cx.stop_propagation();
                on_click(event, window, cx);
            },
        )
}

/// 卡片预览区的占位内容：预览还没解码好、或读不出来时顶上去。
pub(super) fn placeholder(message: &'static str) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .text_size(px(11.0))
                .text_color(text_faint())
                .child(message),
        )
        .into_any_element()
}

/// 截图卡片里三个动作之间的竖线。
pub(super) fn card_divider() -> impl IntoElement {
    div().w(px(1.0)).h(px(18.0)).bg(gpui_kit::rgb(0x2c2c34))
}

/// 「查看格式」的正文：容器、时长、大小，再逐条列轨道。
pub(super) fn render_media_info(info: &crate::player::MediaInfo) -> AnyElement {
    let mut summary: Vec<(String, String)> = Vec::new();
    if !info.container.is_empty() {
        summary.push(("容器".to_string(), info.container.clone()));
    }
    if info.duration > 0.0 {
        summary.push(("时长".to_string(), format_time(info.duration)));
    }
    if let Some(size) = info.size {
        summary.push(("大小".to_string(), format_size(size)));
    }

    div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .children(summary.into_iter().map(|(key, value)| {
            div()
                .flex()
                .justify_between()
                .gap(px(16.0))
                .text_size(px(13.0))
                .child(div().text_color(text_muted()).child(key))
                .child(div().text_color(text()).child(value))
        }))
        .child(
            div()
                .mt(px(6.0))
                .text_size(px(12.0))
                .text_color(text_muted())
                .child(format!("轨道 · {}", info.tracks.len())),
        )
        .children(info.tracks.iter().map(|track| {
            let mut meta: Vec<String> = Vec::new();
            if !track.language.is_empty() {
                meta.push(track.language.to_uppercase());
            }
            if !track.codec.is_empty() {
                meta.push(track.codec.clone());
            }
            if !track.title.is_empty() {
                meta.push(track.title.clone());
            }
            if !track.detail.is_empty() {
                meta.push(track.detail.clone());
            }
            if track.external {
                meta.push("外挂".to_string());
            }
            if meta.is_empty() {
                meta.push("—".to_string());
            }

            div()
                .flex()
                .items_start()
                .gap(px(10.0))
                .text_size(px(13.0))
                .child(div().w(px(34.0)).text_color(ui::accent()).child(track.kind))
                .child(div().flex_1().text_color(text()).child(meta.join(" · ")))
        }))
        .into_any_element()
}

/// 给新列表起名字：文件夹名 / m3u 文件名 / 文件所在目录名。
///
/// 名字是列表在面板上唯一的身份，叫「列表 3」的话开三个页就分不清谁是谁了。
pub(super) fn tab_name_for(paths: &[PathBuf]) -> Option<String> {
    let first = paths.first()?;
    let name = if first.is_dir() {
        first.file_name()
    } else if formats::is_playlist(first) {
        first.file_stem()
    } else {
        first.parent().and_then(|parent| parent.file_name())
    }?;
    let name = name.to_string_lossy().trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// 当前 exe 是不是 `cargo build` 的产物。
///
/// 关联里存的是绝对路径，`target` 目录会被 `cargo clean` 抹掉、被新构建顶替，
/// 所以在用户点下按钮的那一刻值得提一句。
pub(super) fn is_development_build(exe: &std::path::Path) -> bool {
    let Some(directory) = exe.parent() else {
        return false;
    };
    let under_target = directory
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        == Some("target");
    let profile = directory.file_name().and_then(|name| name.to_str());
    under_target && matches!(profile, Some("debug" | "release"))
}

/// 注册项指向的路径是否覆盖当前可执行文件。
///
/// Windows/Linux 上存的就是 exe 本身；macOS 上存的是 `.app` 目录，exe 在它里面，
/// 所以不能直接比相等。
pub(super) fn association_target_matches(target: &std::path::Path) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    exe == target || exe.starts_with(target)
}

/// 「设置」菜单里那行只读状态。
pub(super) fn association_status_line(
    status: &platform::associations::Status,
    supported: bool,
) -> String {
    if !supported {
        return "已注册关联：当前平台尚未支持".to_string();
    }
    if !status.is_registered() {
        return "已注册关联：未注册".to_string();
    }
    let stale = status
        .target
        .as_ref()
        .is_some_and(|target| !association_target_matches(target));
    if stale {
        format!("已注册关联：{} 个格式（路径已过期）", status.registered)
    } else {
        let total = platform::associations::count();
        format!("已注册关联：{}/{} 个格式", status.registered, total)
    }
}
