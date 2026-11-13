//! 片段遮罩：矩形 / 椭圆 / 自由路径，带羽化与位移（M10-T12 的模型层）。
//!
//! **一律用相对坐标**（0–1，按画面宽高各自归一化）：同一个遮罩放在 1080p 与 4K 工程里
//! 构图一致，换序列规格也不用重画。像素只在最后一刻算——导出侧按序列尺寸乘回去。
//!
//! 羽化半径也是相对的（相对短边）：0 是硬边，1 是整幅都虚掉。用绝对像素会让 4K 工程的
//! 羽化看起来只有 1080p 的四分之一。
//!
//! 这里只有数据结构与几何换算，不含滤镜字符串：导出侧要先把遮罩渲成一张 alpha 图再
//! `alphamerge`，那条链属于 `export`，放在这儿会让模型层依赖滤镜细节。

use serde::{Deserialize, Serialize};

/// 遮罩形状。坐标都是画面内的相对值（0–1）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaskShape {
    /// 矩形：左上角 + 宽高。
    Rect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
    /// 椭圆：外接矩形的左上角 + 宽高。
    Ellipse {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
    /// 自由路径：至少三个点，按顺序连成多边形（不闭合时自动闭合）。
    Path(Vec<(f32, f32)>),
}

impl Default for MaskShape {
    fn default() -> Self {
        MaskShape::Rect {
            x: 0.1,
            y: 0.1,
            width: 0.8,
            height: 0.8,
        }
    }
}

impl MaskShape {
    pub fn label(&self) -> &'static str {
        match self {
            MaskShape::Rect { .. } => "矩形",
            MaskShape::Ellipse { .. } => "椭圆",
            MaskShape::Path(_) => "路径",
        }
    }

    /// 顶点数：路径要够三个点才算形状，界面据此禁用「完成」。
    pub fn vertex_count(&self) -> usize {
        match self {
            MaskShape::Path(points) => points.len(),
            _ => 0,
        }
    }

    /// 把越界的坐标夹回画面内。工程是明文 JSON，手改出来的值要能兜住。
    pub fn normalized(&self) -> Self {
        let unit = |value: f32| {
            if value.is_finite() {
                value.clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        match self {
            MaskShape::Rect {
                x,
                y,
                width,
                height,
            } => MaskShape::Rect {
                x: unit(*x),
                y: unit(*y),
                width: unit(*width),
                height: unit(*height),
            },
            MaskShape::Ellipse {
                x,
                y,
                width,
                height,
            } => MaskShape::Ellipse {
                x: unit(*x),
                y: unit(*y),
                width: unit(*width),
                height: unit(*height),
            },
            MaskShape::Path(points) => {
                MaskShape::Path(points.iter().map(|(x, y)| (unit(*x), unit(*y))).collect())
            }
        }
    }

    /// 按序列尺寸换算出像素外接矩形 `(x, y, w, h)`。路径与椭圆都用外接矩形描述范围。
    pub fn pixel_bounds(&self, width: u32, height: u32) -> (u32, u32, u32, u32) {
        let (width_f, height_f) = (width as f32, height as f32);
        let (left, top, right, bottom) = match self {
            MaskShape::Rect {
                x,
                y,
                width: w,
                height: h,
            }
            | MaskShape::Ellipse {
                x,
                y,
                width: w,
                height: h,
            } => (*x, *y, x + w, y + h),
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
                (left, top, right, bottom)
            }
        };
        // 偶数宽高：多数编码器要 4:2:0 的偶数尺寸，奇数会在边缘留一行黑。
        let x = (left.clamp(0.0, 1.0) * width_f).round() as u32;
        let y = (top.clamp(0.0, 1.0) * height_f).round() as u32;
        let w = ((right - left).clamp(0.0, 1.0) * width_f).round() as u32 & !1;
        let h = ((bottom - top).clamp(0.0, 1.0) * height_f).round() as u32 & !1;
        (
            x.min(width.saturating_sub(2)),
            y.min(height.saturating_sub(2)),
            w.max(2).min(width.saturating_sub(x)),
            h.max(2).min(height.saturating_sub(y)),
        )
    }
}

/// 一个遮罩：形状 + 羽化 + 位移。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mask {
    pub enabled: bool,
    pub shape: MaskShape,
    /// 羽化半径，相对短边（0 = 硬边）。
    pub feather: f32,
    /// 整体位移，相对画面宽高；关键帧化的是位移，不是形状本身。
    pub offset: (f32, f32),
}

impl Default for Mask {
    fn default() -> Self {
        Self {
            enabled: false,
            shape: MaskShape::default(),
            feather: 0.0,
            offset: (0.0, 0.0),
        }
    }
}

/// 羽化上限：超过 0.5 整幅就糊成一片，没有意义。
pub const MAX_FEATHER: f32 = 0.5;

impl Mask {
    /// 夹好范围后的羽化半径。
    pub fn normalized_feather(&self) -> f32 {
        if self.feather.is_finite() {
            self.feather.clamp(0.0, MAX_FEATHER)
        } else {
            0.0
        }
    }

    /// 位移换算成像素 `(dx, dy)`。
    pub fn offset_pixels(&self, width: u32, height: u32) -> (i32, i32) {
        let unit = |value: f32, span: u32| {
            if value.is_finite() {
                (value.clamp(-1.0, 1.0) * span as f32).round() as i32
            } else {
                0
            }
        };
        (unit(self.offset.0, width), unit(self.offset.1, height))
    }

    /// 这个遮罩会不会改变画面：没启用、或者形状退化（路径点不足三个）都算「不做」。
    ///
    /// 导出侧据此决定要不要插那条 `alphamerge` 链：为一张无效遮罩多跑一遍合成
    /// 是纯粹的浪费。
    pub fn effective(&self) -> bool {
        if !self.enabled {
            return false;
        }
        match &self.shape {
            MaskShape::Path(points) => points.len() >= 3,
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests;
