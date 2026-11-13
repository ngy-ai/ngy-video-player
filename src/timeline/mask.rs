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

    /// 某个像素的覆盖率（0 = 全透明，1 = 全不透明）。
    ///
    /// 羽化按「到边界的距离」算：边界上正好 0.5，往里走 `feather/2` 个像素到 1，
    /// 往外走同样距离到 0。这样羽化半径就是「从全透明到全不透明」的宽度，
    /// 与界面上的滑块对得上。`feather` 是相对短边的比例，这里换算成像素。
    pub fn alpha_at(&self, px: f32, py: f32, width: u32, height: u32) -> f32 {
        let (dx, dy) = self.offset_pixels(width, height);
        let x = px - dx as f32;
        let y = py - dy as f32;
        let shape = self.shape.normalized();
        // 有符号距离：正数表示在形状里面，单位是像素。
        let distance = match &shape {
            MaskShape::Rect {
                x: left,
                y: top,
                width: w,
                height: h,
            } => {
                let (x0, y0) = (left * width as f32, top * height as f32);
                let (x1, y1) = (x0 + w * width as f32, y0 + h * height as f32);
                (x - x0).min(x1 - x).min(y - y0).min(y1 - y)
            }
            MaskShape::Ellipse {
                x: left,
                y: top,
                width: w,
                height: h,
            } => {
                let (cx, cy) = (
                    (left + w * 0.5) * width as f32,
                    (top + h * 0.5) * height as f32,
                );
                let (rx, ry) = (
                    (w * width as f32 * 0.5).max(0.5),
                    (h * height as f32 * 0.5).max(0.5),
                );
                let (u, v) = ((x - cx) / rx, (y - cy) / ry);
                // 归一化距离乘以短半轴，回到像素量级；椭圆边上正好 0。
                (1.0 - (u * u + v * v).sqrt()) * rx.min(ry)
            }
            MaskShape::Path(points) => {
                // 退化路径（不够三点）没有内外之分：整幅透明，导出侧也会跳过它。
                if points.len() < 3 {
                    return 0.0;
                }
                let scaled: Vec<(f32, f32)> = points
                    .iter()
                    .map(|(x, y)| (x * width as f32, y * height as f32))
                    .collect();
                let inside = point_in_polygon(x, y, &scaled);
                let mut nearest = f32::MAX;
                for (index, (ax, ay)) in scaled.iter().enumerate() {
                    let (bx, by) = scaled[(index + 1) % scaled.len()];
                    nearest = nearest.min(distance_to_segment(x, y, *ax, *ay, bx, by));
                }
                if inside { nearest } else { -nearest }
            }
        };

        let feather = self.normalized_feather() * width.min(height) as f32;
        if feather <= 0.5 {
            return if distance >= 0.0 { 1.0 } else { 0.0 };
        }
        (distance / feather + 0.5).clamp(0.0, 1.0)
    }

    /// alpha 图，一行像素接一行，8 位灰度（255 = 不透明）。
    ///
    /// 导出侧把它写成 PGM 再 `alphamerge`：与其在滤镜表达式里凑出多边形，
    /// 不如一次算好这张图——路径遮罩的表达式根本写不出来。
    pub fn alpha_plane(&self, width: u32, height: u32) -> Vec<u8> {
        let mut plane = Vec::with_capacity((width * height) as usize);
        for row in 0..height {
            for column in 0..width {
                // 取像素中心：边缘那一列的覆盖率才算得准。
                let alpha = self.alpha_at(column as f32 + 0.5, row as f32 + 0.5, width, height);
                plane.push((alpha * 255.0).round() as u8);
            }
        }
        plane
    }

    /// 上面那张图的 PGM（P5 二进制）字节。ffmpeg 直接认这个格式，解出来是灰度。
    pub fn pgm(&self, width: u32, height: u32) -> Vec<u8> {
        let plane = self.alpha_plane(width, height);
        let header = format!("P5\n{width} {height}\n255\n");
        let mut bytes = Vec::with_capacity(header.len() + plane.len());
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(&plane);
        bytes
    }
}

/// 射线法判断点是否在多边形内（不闭合的多边形按闭合处理）。
fn point_in_polygon(x: f32, y: f32, points: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let count = points.len();
    if count < 3 {
        return false;
    }
    let mut j = count - 1;
    for i in 0..count {
        let (xi, yi) = points[i];
        let (xj, yj) = points[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// 点到线段的距离（像素）。
fn distance_to_segment(x: f32, y: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let length = dx * dx + dy * dy;
    if length <= f32::EPSILON {
        return ((x - ax).powi(2) + (y - ay).powi(2)).sqrt();
    }
    let t = (((x - ax) * dx + (y - ay) * dy) / length).clamp(0.0, 1.0);
    let (px, py) = (ax + t * dx, ay + t * dy);
    ((x - px).powi(2) + (y - py).powi(2)).sqrt()
}

#[cfg(test)]
mod tests;
