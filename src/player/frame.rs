//! mpv 软件渲染的像素缓冲：mpv 把一帧 BGRA 写进来，再转成 GPUI 的 `RenderImage`。
//!
//! 主画面和进度条预览走的是同一条路（渲染上下文 + 一块对齐内存），差别只有尺寸。
//! 放在一处是为了别把「对齐、逐行拷出、补 alpha」这些细节写两遍——其中任何一条
//! 写错，表现都是画面全黑或偶发撕裂，排查成本远高于共用这段代码。

use crate::player::ffi::{MpvApi, MpvRenderContext, MpvRenderParam, render_param};
use anyhow::{Result, anyhow};
use gpui_kit::RenderImage;
use image::{Frame, ImageBuffer, Rgba};
use std::ffi::{c_int, c_void};

/// 一块 64 字节对齐的取帧缓冲。
///
/// mpv 用它内部的 SIMD 路径直接写这块内存；只要 stride 与起始指针都对齐到 64，
/// 就不会触发「先渲染到临时缓冲、再整帧 memcpy 给你」的退化路径。
#[derive(Default)]
pub(crate) struct FrameBuffer {
    storage: Vec<u8>,
    offset: usize,
    stride: usize,
    width: u32,
    height: u32,
}

impl FrameBuffer {
    /// 按目标尺寸重新对齐。尺寸不变时只更新偏移，不重新分配。
    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        // 每行按 64 字节向上取整（4 字节像素，64/4=16，始终是整数个像素）。
        let stride = ((width as usize * 4).div_ceil(64)) * 64;
        let needed = stride * height as usize + 64;
        if self.storage.len() < needed {
            self.storage.resize(needed, 0);
        }
        let base = self.storage.as_ptr() as usize;
        self.offset = (64 - (base % 64)) % 64;
        self.stride = stride;
        self.width = width;
        self.height = height;
    }

    fn pointer(&mut self) -> *mut u8 {
        unsafe { self.storage.as_mut_ptr().add(self.offset) }
    }

    /// 拷成一张紧密排列、alpha 全不透明的 BGRA 图像，并按 `rotation` 顺时针转。
    ///
    /// 三个细节：
    /// * mpv 给的第四字节是「未定义的填充值」，必须强制写成 255。不为 0 的话
    ///   GPUI 的 alpha 混合会把整帧当成透明，屏幕上只剩黑底。
    /// * 这次拷贝无法避免：`RenderImage` 要求拥有自己的像素；旋转顺路做在这一次
    ///   拷贝里，不再多开一块缓冲。
    /// * 90/270 度时输出图的长宽互换，调用方按图片实际尺寸排版即可。
    pub(crate) fn to_image(&self, rotation: u16) -> RenderImage {
        let width = self.width as usize;
        let height = self.height as usize;
        let rotation = rotation % 360;
        let swapped = rotation == 90 || rotation == 270;
        let (out_width, out_height) = if swapped {
            (height, width)
        } else {
            (width, height)
        };

        let mut pixels = vec![0u8; out_width * out_height * 4];
        for y in 0..height {
            let src_start = self.offset + y * self.stride;
            let src = &self.storage[src_start..src_start + width * 4];
            match rotation {
                // 顺时针 90°：(x, y) → (height-1-y, x)，此时输出宽 = 输入高。
                90 => {
                    for x in 0..width {
                        let dst = (x * out_width + (height - 1 - y)) * 4;
                        pixels[dst..dst + 4].copy_from_slice(&src[x * 4..x * 4 + 4]);
                    }
                }
                // 180°：整行倒着写，行内像素也倒序。
                180 => {
                    let row = (height - 1 - y) * out_width * 4;
                    for x in 0..width {
                        let dst = row + (width - 1 - x) * 4;
                        pixels[dst..dst + 4].copy_from_slice(&src[x * 4..x * 4 + 4]);
                    }
                }
                // 顺时针 270°（= 逆时针 90°）：(x, y) → (y, width-1-x)。
                270 => {
                    for x in 0..width {
                        let dst = ((width - 1 - x) * out_width + y) * 4;
                        pixels[dst..dst + 4].copy_from_slice(&src[x * 4..x * 4 + 4]);
                    }
                }
                _ => {
                    let dst = y * out_width * 4;
                    pixels[dst..dst + width * 4].copy_from_slice(src);
                }
            }
        }
        for chunk in pixels.chunks_exact_mut(4) {
            chunk[3] = 255;
        }

        let image: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_raw(out_width as u32, out_height as u32, pixels)
                .expect("缓冲区尺寸由我们自己分配，必然匹配");
        RenderImage::new([Frame::new(image)])
    }
}

/// 让 mpv 把当前帧画进 `buffer`。尺寸必须已经 `resize` 过，且调用前
/// `mpv_render_context_update()` 报告过有新帧。
pub(crate) fn render_sw(
    api: &MpvApi,
    context: *mut MpvRenderContext,
    width: u32,
    height: u32,
    buffer: &mut FrameBuffer,
) -> Result<()> {
    let mut dimensions: [c_int; 2] = [width as c_int, height as c_int];
    let mut stride: usize = buffer.stride;
    // 注意：SW_POINTER 的 data 就是像素数据本身的地址（`Type: void*`），
    // 不是「指向指针的指针」。多套一层星号的话 mpv 会把画面写到栈上这个变量
    // 所在的地址，直接段错误。
    let pointer: *mut c_void = buffer.pointer() as *mut c_void;
    // mpv 只读这个字符串，用静态字面量就够，不必每帧分配一个 CString。
    let format = b"bgr0\0";

    let mut params = [
        MpvRenderParam {
            kind: render_param::SW_SIZE,
            data: dimensions.as_mut_ptr() as *mut c_void,
        },
        MpvRenderParam {
            kind: render_param::SW_FORMAT,
            data: format.as_ptr() as *mut c_void,
        },
        MpvRenderParam {
            kind: render_param::SW_STRIDE,
            data: &mut stride as *mut usize as *mut c_void,
        },
        MpvRenderParam {
            kind: render_param::SW_POINTER,
            data: pointer,
        },
        MpvRenderParam {
            kind: render_param::INVALID,
            data: std::ptr::null_mut(),
        },
    ];

    let rc = unsafe { (api.render_context_render)(context, params.as_mut_ptr()) };
    if rc < 0 {
        return Err(anyhow!("mpv_render_context_render: {}", api.error_text(rc)));
    }
    Ok(())
}
