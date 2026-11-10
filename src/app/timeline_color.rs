//! 片段调色面板：亮度 / 对比度 / 饱和度 / 色温四行步进，加一个 3D LUT。
//!
//! 参数是整数百分比（-100..=100），按一下就 ±10：步进比滑块好点（不用瞄准），
//! 也比输入框好撤销（每次点击落一步）。LUT 是文件，走原生选择框。
//!
//! 面板保持打开：校色要来回试，点一下就得重新右键一次太烦。

use super::*;
use crate::timeline::{COLOR_RANGE, COLOR_STEP, ColorGrade, EditOp, Id};

/// 面板上下文：作用在哪个片段、贴在哪里。
#[derive(Clone, Copy, Debug)]
pub(super) struct ColorMenu {
    clip: Id,
    position: Point<Pixels>,
}

const PANEL_WIDTH: f32 = 320.0;
/// 标题 + 摘要 + 四行步进 + LUT 行 + 说明，只用来判断在鼠标下方放不放得下。
const PANEL_HEIGHT: f32 = 248.0;

impl PlayerApp {
    /// 打开片段的调色面板。片段已经不在时什么都不做。
    pub(super) fn open_color_menu(
        &mut self,
        clip: Id,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        if project.sequence.clip(clip).is_none() {
            return;
        }
        self.timeline_menu = Some(TimelineMenu::Color(ColorMenu { clip, position }));
        cx.notify();
    }

    /// 把一整套调色参数落到片段上。值没变时不落撤销。
    fn apply_color(&mut self, clip: Id, color: ColorGrade, cx: &mut Context<Self>) {
        let message = format!("调色：{}", color.summary());
        let result = self.project.as_ref().map(|project| {
            crate::timeline::apply(
                &project.sequence,
                &EditOp::SetColor {
                    clip,
                    color: color.clone(),
                },
            )
        });
        match result {
            Some(Ok(next)) => {
                if self
                    .project
                    .as_ref()
                    .is_some_and(|project| project.sequence == next)
                {
                    self.flash("没有变化");
                    cx.notify();
                    return;
                }
                self.commit_edit(next, message.clone());
                self.flash(message);
                self.poke();
                cx.notify();
            }
            Some(Err(error)) => {
                self.flash(error.to_string());
                cx.notify();
            }
            None => {}
        }
    }

    /// 在当前参数上改一处，再交给 [`Self::apply_color`]。
    fn adjust_color(
        &mut self,
        clip: Id,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut ColorGrade),
    ) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let Some(item) = project.sequence.clip(clip) else {
            return;
        };
        let mut color = item.color.clone();
        change(&mut color);
        // 夹到量程：手改坏的工程也能在面板里按回来。
        color.brightness = color.brightness.clamp(-COLOR_RANGE, COLOR_RANGE);
        color.contrast = color.contrast.clamp(-COLOR_RANGE, COLOR_RANGE);
        color.saturation = color.saturation.clamp(-COLOR_RANGE, COLOR_RANGE);
        color.temperature = color.temperature.clamp(-COLOR_RANGE, COLOR_RANGE);
        self.apply_color(clip, color, cx);
    }

    /// 选一个 3D LUT 文件。存的是相对工程目录的路径（跨盘符才存绝对）。
    fn pick_color_lut(&mut self, clip: Id, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open {
            return;
        }
        let Some(project) = self.project.as_ref() else {
            return;
        };
        // 从工程目录开始找：LUT 通常和工程放在一起。
        let directory = if project.base_dir().as_os_str().is_empty() {
            self.settings.last_dir.clone()
        } else {
            Some(project.base_dir().to_path_buf())
        };

        self.dialog_open = true;
        let mut picker = rfd::FileDialog::new()
            .add_filter("3D LUT", &["cube", "3dl", "dat", "m3d"])
            .set_parent(window);
        if let Some(directory) = directory {
            picker = picker.set_directory(directory);
        }

        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(picker.pick_file());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if let Some(path) = picked {
                    let stored = this
                        .project
                        .as_ref()
                        .map(|project| project.portable_path(&path))
                        .unwrap_or(path);
                    this.adjust_color(clip, cx, |color| color.lut = Some(stored));
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 调色面板。
    pub(super) fn render_color_menu(
        &self,
        menu: ColorMenu,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let project = self.project.as_ref()?;
        let clip = project.sequence.clip(menu.clip)?;
        let color = clip.color.clone();
        let title = format!(
            "调色 · {}",
            super::timeline_effects::clip_title(project, clip)
        );
        let origin = super::timeline_menu::menu_origin(menu.position, PANEL_HEIGHT, window);
        let clip_id = menu.clip;

        let lut_label = match color.lut.as_ref() {
            Some(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            None => "未加载 LUT".to_string(),
        };
        let has_lut = color.lut.is_some();

        let rows = div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(color_row(
                "color-brightness-minus",
                "color-brightness-plus",
                "亮度",
                color.brightness,
                clip_id,
                cx,
                |color, value| color.brightness = value,
            ))
            .child(color_row(
                "color-contrast-minus",
                "color-contrast-plus",
                "对比度",
                color.contrast,
                clip_id,
                cx,
                |color, value| color.contrast = value,
            ))
            .child(color_row(
                "color-saturation-minus",
                "color-saturation-plus",
                "饱和度",
                color.saturation,
                clip_id,
                cx,
                |color, value| color.saturation = value,
            ))
            .child(color_row(
                "color-temperature-minus",
                "color-temperature-plus",
                "色温",
                color.temperature,
                clip_id,
                cx,
                |color, value| color.temperature = value,
            ));

        let lut_row = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .h(px(26.0))
            .child(row_label("LUT"))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .text_size(px(11.0))
                    .text_color(if has_lut { text() } else { text_faint() })
                    .child(ui::ellipsis(lut_label)),
            )
            .child(chip(
                "color-lut-pick",
                "选择…",
                false,
                cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.pick_color_lut(clip_id, window, cx);
                }),
            ))
            .child(chip(
                "color-lut-clear",
                "清除",
                false,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.adjust_color(clip_id, cx, |color| color.lut = None);
                }),
            ))
            .child(chip(
                "color-reset",
                "复原",
                false,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.adjust_color(clip_id, cx, |color| *color = ColorGrade::default());
                }),
            ));

        Some(
            div()
                .id("timeline-color-menu")
                .absolute()
                .left(origin.x)
                .top(origin.y)
                .w(px(PANEL_WIDTH))
                .py(px(8.0))
                .px(px(10.0))
                .rounded(px(10.0))
                .bg(surface())
                .border_1()
                .border_color(gpui_kit::rgb(0x2c2c34))
                .shadow_lg()
                .flex()
                .flex_col()
                .gap(px(5.0))
                // 与其它面板一样：点面板本身不能漏到下面的时间线去。
                .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_this, _: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                    }),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(text_muted())
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(text_faint())
                        .child(color.summary()),
                )
                .child(rows)
                .child(lut_row)
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(text_faint())
                        .child("导出走 eq → colorbalance → lut3d；预览里看不到调色"),
                )
                .into_any_element(),
        )
    }
}

/// 一行「标签 + － + 数值 + ＋」。`field` 决定改的是哪一个参数。
///
/// 两颗按钮的 id 由调用方给静态串：GPUI 靠 id 做命中测试，每次渲染换一个就等于换了元素。
fn color_row(
    minus_id: &'static str,
    plus_id: &'static str,
    label: &'static str,
    value: i32,
    clip: Id,
    cx: &mut Context<PlayerApp>,
    field: fn(&mut ColorGrade, i32),
) -> AnyElement {
    let lower = (value - COLOR_STEP).clamp(-COLOR_RANGE, COLOR_RANGE);
    let higher = (value + COLOR_STEP).clamp(-COLOR_RANGE, COLOR_RANGE);
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .h(px(26.0))
        .child(row_label(label))
        .child(step_button(
            minus_id,
            "－",
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.adjust_color(clip, cx, |color| field(color, lower));
            }),
        ))
        .child(
            div()
                .w(px(44.0))
                .text_size(px(11.0))
                .text_color(if value == 0 {
                    text_faint()
                } else {
                    text_muted()
                })
                .child(format_signed(value)),
        )
        .child(step_button(
            plus_id,
            "＋",
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.adjust_color(clip, cx, |color| field(color, higher));
            }),
        ))
        .into_any_element()
}

/// 步进小按钮：面板里一行放进两颗，尺寸比 `chip` 更窄。
fn step_button(
    id: &'static str,
    label: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(24.0))
        .h(px(22.0))
        .rounded(px(5.0))
        .bg(ui::track())
        .text_size(px(11.0))
        .text_color(text_muted())
        .cursor(CursorStyle::PointingHand)
        .hover(|style| style.bg(ui::surface_hover()).text_color(text()))
        .child(label)
        .on_click(
            move |event: &ClickEvent, window: &mut Window, cx: &mut App| {
                cx.stop_propagation();
                on_click(event, window, cx);
            },
        )
        .into_any_element()
}

/// 行标签：固定宽度，四行的 −/＋ 才对得齐。
fn row_label(text: &'static str) -> AnyElement {
    div()
        .w(px(44.0))
        .flex_none()
        .text_size(px(11.0))
        .text_color(text_muted())
        .child(text)
        .into_any_element()
}

/// 数值写法：0 就是 `0`，其余带符号——`+20` / `-30` 一眼能看出往哪个方向调。
fn format_signed(value: i32) -> String {
    if value == 0 {
        "0".to_string()
    } else {
        format!("{value:+}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 数值带符号显示() {
        assert_eq!(format_signed(0), "0");
        assert_eq!(format_signed(20), "+20");
        assert_eq!(format_signed(-30), "-30");
    }

    #[test]
    fn 步进在两端被夹住() {
        let top = (COLOR_RANGE - COLOR_STEP + COLOR_STEP).clamp(-COLOR_RANGE, COLOR_RANGE);
        assert_eq!(top, COLOR_RANGE);
        let bottom = (-COLOR_RANGE + COLOR_STEP - COLOR_STEP).clamp(-COLOR_RANGE, COLOR_RANGE);
        assert_eq!(bottom, -COLOR_RANGE);
    }
}
