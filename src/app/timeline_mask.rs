//! 片段遮罩面板：矩形 / 椭圆的开关、大小、羽化与位移。
//!
//! 形状只用相对坐标（0–1），面板里的每一步都按画面比例改——4K 与 1080p 的工程
//! 拉动同样的步数，看到的构图一致。每次点击落一步撤销，走 `EditOp::SetMask`。
//!
//! 自由路径（`MaskShape::Path`）这里只能看不能画：多点图形编辑要在预览上点，
//! 属于另一件事（README「已知限制」里写明了）。工程里存着的路径遮罩照样导出。
//!
//! 面板保持打开：找遮罩边界要来回试几档。

use super::*;
use crate::timeline::{EditOp, Id, Mask, MaskShape};

/// 面板上下文：作用在哪个片段、贴在哪里。
#[derive(Clone, Copy, Debug)]
pub(super) struct MaskMenu {
    clip: Id,
    position: Point<Pixels>,
}

const PANEL_WIDTH: f32 = 320.0;
/// 标题 + 摘要 + 开关 + 形状 + 大小 + 羽化 + 位移 + 说明，只用来判断放不放得下。
const PANEL_HEIGHT: f32 = 262.0;

/// 大小的步进（相对画面边长）。
const SIZE_STEP: f32 = 0.05;
/// 羽化的步进（相对短边）。
const FEATHER_STEP: f32 = 0.05;
/// 位移的步进（相对画面宽高）。
const OFFSET_STEP: f32 = 0.02;

impl PlayerApp {
    /// 打开片段的遮罩面板。片段已经不在时什么都不做。
    pub(super) fn open_mask_menu(
        &mut self,
        clip: Id,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        if project.sequence().clip(clip).is_none() {
            return;
        }
        self.timeline_menu = Some(TimelineMenu::Mask(MaskMenu { clip, position }));
        cx.notify();
    }

    /// 把一整套遮罩参数落到片段上。值没变时不落撤销。
    fn apply_mask(&mut self, clip: Id, mask: Mask, cx: &mut Context<Self>) {
        let message = format!("遮罩：{}", mask_summary(&mask));
        let result = self.project.as_ref().map(|project| {
            crate::timeline::apply(project.sequence(), &EditOp::SetMask { clip, mask })
        });
        match result {
            Some(Ok(next)) => {
                if self
                    .project
                    .as_ref()
                    .is_some_and(|project| *project.sequence() == next)
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

    /// 在当前参数上改一处，再交给 [`Self::apply_mask`]。
    fn adjust_mask(&mut self, clip: Id, cx: &mut Context<Self>, change: impl FnOnce(&mut Mask)) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let Some(item) = project.sequence().clip(clip) else {
            return;
        };
        let mut mask = item.mask.clone();
        change(&mut mask);
        self.apply_mask(clip, mask, cx);
    }

    /// 遮罩面板。
    pub(super) fn render_mask_menu(
        &self,
        menu: MaskMenu,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let project = self.project.as_ref()?;
        let clip = project.sequence().clip(menu.clip)?;
        let mask = clip.mask.clone();
        let title = format!(
            "遮罩 · {}",
            super::timeline_effects::clip_title(project, clip)
        );
        let origin = super::timeline_menu::menu_origin(menu.position, PANEL_HEIGHT, window);
        let clip_id = menu.clip;
        let enabled = mask.enabled;
        let is_ellipse = matches!(mask.shape, MaskShape::Ellipse { .. });
        let locked = project
            .sequence()
            .clip_track(clip_id)
            .and_then(|track| project.sequence().track(track))
            .is_some_and(|track| track.locked);

        let toggle = chip(
            "mask-toggle",
            if enabled {
                "关闭遮罩"
            } else {
                "启用遮罩"
            },
            enabled,
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.adjust_mask(clip_id, cx, |mask| mask.enabled = !mask.enabled);
            }),
        );

        let shapes = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .h(px(26.0))
            .child(row_label("形状"))
            .child(chip(
                "mask-shape-rect",
                "矩形",
                !is_ellipse,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.adjust_mask(clip_id, cx, |mask| {
                        mask.shape = with_shape(mask, false);
                    });
                }),
            ))
            .child(chip(
                "mask-shape-ellipse",
                "椭圆",
                is_ellipse,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.adjust_mask(clip_id, cx, |mask| {
                        mask.shape = with_shape(mask, true);
                    });
                }),
            ));

        let size = step_row(
            "mask-size-smaller",
            "mask-size-bigger",
            "大小",
            format!("{:.0}%", size_percent(&mask)),
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.adjust_mask(clip_id, cx, |mask| resize_mask(mask, -SIZE_STEP));
            }),
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.adjust_mask(clip_id, cx, |mask| resize_mask(mask, SIZE_STEP));
            }),
        );

        let feather = step_row(
            "mask-feather-less",
            "mask-feather-more",
            "羽化",
            format!("{:.0}%", mask.normalized_feather() * 100.0),
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.adjust_mask(clip_id, cx, |mask| {
                    mask.feather = (mask.normalized_feather() - FEATHER_STEP).max(0.0);
                });
            }),
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.adjust_mask(clip_id, cx, |mask| {
                    mask.feather = (mask.normalized_feather() + FEATHER_STEP)
                        .min(crate::timeline::mask::MAX_FEATHER);
                });
            }),
        );

        let (dx, dy) = mask.offset;
        let offset_row = div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .h(px(26.0))
            .child(row_label("位移"))
            .child(step_chip(
                "mask-offset-left",
                "←",
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.adjust_mask(clip_id, cx, |mask| {
                        mask.offset.0 -= OFFSET_STEP;
                    });
                }),
            ))
            .child(step_chip(
                "mask-offset-right",
                "→",
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.adjust_mask(clip_id, cx, |mask| {
                        mask.offset.0 += OFFSET_STEP;
                    });
                }),
            ))
            .child(step_chip(
                "mask-offset-up",
                "↑",
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.adjust_mask(clip_id, cx, |mask| {
                        mask.offset.1 -= OFFSET_STEP;
                    });
                }),
            ))
            .child(step_chip(
                "mask-offset-down",
                "↓",
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.adjust_mask(clip_id, cx, |mask| {
                        mask.offset.1 += OFFSET_STEP;
                    });
                }),
            ))
            .child(
                div()
                    .w(px(58.0))
                    .text_size(px(11.0))
                    .text_color(if dx == 0.0 && dy == 0.0 {
                        text_faint()
                    } else {
                        text_muted()
                    })
                    .child(format!("{:.0}%, {:.0}%", dx * 100.0, dy * 100.0)),
            )
            .child(chip(
                "mask-reset",
                "复原",
                false,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.adjust_mask(clip_id, cx, |mask| {
                        let enabled = mask.enabled;
                        *mask = Mask::default();
                        mask.enabled = enabled;
                    });
                }),
            ));

        let footer = if locked {
            "轨道已锁定：先解锁再改".to_string()
        } else {
            "导出走 alphamerge，遮罩外压成黑；预览里看不到遮罩".to_string()
        };

        Some(
            div()
                .id("timeline-mask-menu")
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
                        .child(format!("{} · {}", mask.shape.label(), mask_summary(&mask))),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .h(px(26.0))
                        .child(row_label("开关"))
                        .child(toggle),
                )
                .child(shapes)
                .child(size)
                .child(feather)
                .child(offset_row)
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(text_faint())
                        .child(footer),
                )
                .into_any_element(),
        )
    }
}

/// 换形状但保留范围：矩形切成椭圆时，用户不会想重画一遍。
fn with_shape(mask: &Mask, ellipse: bool) -> MaskShape {
    let (x, y, width, height) = bounds_of(&mask.shape);
    if ellipse {
        MaskShape::Ellipse {
            x,
            y,
            width,
            height,
        }
    } else {
        MaskShape::Rect {
            x,
            y,
            width,
            height,
        }
    }
}

/// 形状的外接矩形（相对坐标）。路径取外接矩形，换成矩形 / 椭圆时用得上。
fn bounds_of(shape: &MaskShape) -> (f32, f32, f32, f32) {
    match shape {
        MaskShape::Rect {
            x,
            y,
            width,
            height,
        }
        | MaskShape::Ellipse {
            x,
            y,
            width,
            height,
        } => (*x, *y, *width, *height),
        MaskShape::Path(points) => {
            let mut left = 1.0f32;
            let mut top = 1.0f32;
            let mut right = 0.0f32;
            let mut bottom = 0.0f32;
            for (x, y) in points {
                left = left.min(*x);
                top = top.min(*y);
                right = right.max(*x);
                bottom = bottom.max(*y);
            }
            (left, top, right - left, bottom - top)
        }
    }
}

/// 以画面中心为锚点缩放外接矩形：改大小时用户想要的是「往外扩一圈」，
/// 而不是「左上角不动、右下角跑掉」。
fn resize_mask(mask: &mut Mask, delta: f32) {
    let (x, y, width, height) = bounds_of(&mask.shape);
    let (center_x, center_y) = (x + width * 0.5, y + height * 0.5);
    // 最小 5%：再小就整幅都是透明的，等价于关掉。
    let width = (width + delta).clamp(0.05, 1.0);
    let height = (height + delta).clamp(0.05, 1.0);
    // 夹回画面内：整体挪回来而不是只缩一边，形状不会被压扁。
    let x = (center_x - width * 0.5).clamp(0.0, 1.0 - width);
    let y = (center_y - height * 0.5).clamp(0.0, 1.0 - height);
    mask.shape = if matches!(mask.shape, MaskShape::Ellipse { .. }) {
        MaskShape::Ellipse {
            x,
            y,
            width,
            height,
        }
    } else {
        MaskShape::Rect {
            x,
            y,
            width,
            height,
        }
    };
}

/// 面板上显示的大小：外接矩形的百分比（两个方向取小的那个）。
fn size_percent(mask: &Mask) -> f32 {
    let (_, _, width, height) = bounds_of(&mask.shape);
    width.max(height) * 100.0
}

/// 摘要文案。
fn mask_summary(mask: &Mask) -> String {
    if !mask.enabled {
        return "未启用".to_string();
    }
    let (_, _, width, height) = bounds_of(&mask.shape);
    let feather = mask.normalized_feather();
    if feather > 0.001 {
        format!(
            "{:.0}%×{:.0}%，羽化 {:.0}%",
            width * 100.0,
            height * 100.0,
            feather * 100.0
        )
    } else {
        format!("{:.0}%×{:.0}%", width * 100.0, height * 100.0)
    }
}

/// 一行「标签 + － + 数值 + ＋」。
fn step_row(
    minus_id: &'static str,
    plus_id: &'static str,
    label: &'static str,
    value: String,
    minus: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    plus: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let width = if value.len() > 6 { 66.0 } else { 52.0 };
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .h(px(26.0))
        .child(row_label(label))
        .child(step_chip(minus_id, "－", minus))
        .child(
            div()
                .w(px(width))
                .text_size(px(11.0))
                .text_color(text_muted())
                .child(value),
        )
        .child(step_chip(plus_id, "＋", plus))
        .into_any_element()
}

/// 步进小按钮。
fn step_chip(
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

/// 行标签：固定宽度，几行的按钮才对得齐。
fn row_label(text: &'static str) -> AnyElement {
    div()
        .w(px(44.0))
        .flex_none()
        .text_size(px(11.0))
        .text_color(text_muted())
        .child(text)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect_mask() -> Mask {
        Mask {
            enabled: true,
            shape: MaskShape::Rect {
                x: 0.2,
                y: 0.2,
                width: 0.6,
                height: 0.6,
            },
            feather: 0.0,
            offset: (0.0, 0.0),
        }
    }

    #[test]
    fn 换形状保留范围() {
        let mask = rect_mask();
        let ellipse = with_shape(&mask, true);
        assert_eq!(bounds_of(&ellipse), bounds_of(&mask.shape));
        // 再换回来还是同一个矩形。
        let back = with_shape(
            &Mask {
                shape: ellipse,
                ..mask.clone()
            },
            false,
        );
        assert_eq!(bounds_of(&back), (0.2, 0.2, 0.6, 0.6));
    }

    #[test]
    fn 缩放围着中心长() {
        let mut mask = rect_mask();
        resize_mask(&mut mask, 0.1);
        let (x, y, width, height) = bounds_of(&mask.shape);
        // 中心不动：0.5 还是 0.5。
        assert!((x + width * 0.5 - 0.5).abs() < 1e-6);
        assert!((y + height * 0.5 - 0.5).abs() < 1e-6);
        assert!((width - 0.7).abs() < 1e-6);
    }

    #[test]
    fn 缩放不出画面且留最小尺寸() {
        let mut mask = rect_mask();
        // 贴着右下角往外长：夹回画面内，尺寸不被压扁。
        mask.shape = MaskShape::Rect {
            x: 0.6,
            y: 0.6,
            width: 0.4,
            height: 0.4,
        };
        resize_mask(&mut mask, 0.5);
        let (x, y, width, height) = bounds_of(&mask.shape);
        assert!(x >= 0.0 && y >= 0.0);
        assert!(x + width <= 1.0 + 1e-6, "{}", x + width);
        assert!((width - height).abs() < 1e-6, "两个方向一起长");

        let mut tiny = rect_mask();
        for _ in 0..40 {
            resize_mask(&mut tiny, -SIZE_STEP);
        }
        let (_, _, width, _) = bounds_of(&tiny.shape);
        assert!(width >= 0.05 - 1e-6, "{width}");
    }

    #[test]
    fn 摘要写清尺寸与羽化() {
        let mut mask = rect_mask();
        assert_eq!(mask_summary(&mask), "60%×60%");
        mask.feather = 0.1;
        assert_eq!(mask_summary(&mask), "60%×60%，羽化 10%");
        mask.enabled = false;
        assert_eq!(mask_summary(&mask), "未启用");
    }
}
