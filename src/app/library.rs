//! 素材库面板：工程左侧那一栏，列出工程里登记过的所有素材。
//!
//! 它是「从零编排」的第一步：先把要用的东西收进来，再往轨道上放（M9-T02）。
//! 因此这里只做四件事——导入、搜索、移除、试听原片；**不碰时间线**。双击素材
//! 只是把预览区临时切到原片，播放头与序列内容一概不动。
//!
//! 两条慢活都在后台线程上：
//!
//! - **探测**（`ffmpeg -i`）：导入一个几百个文件的文件夹时，同步跑会把界面卡死。
//!   主线程先登记素材（拿到 id），探测结果通过 `futures::mpsc` 回来再写进素材表。
//! - **缩略图**：一个常驻线程串行跑 ffmpeg 生成图块。串行是刻意的——两百个文件
//!   同时起两百个 ffmpeg 只会互相抢 IO，第一张图反而更晚出来。
//!
//! 素材状态（缺失 / 被换过）要靠 `fs::metadata` 判断，不能放进每帧都跑的绘制路径：
//! 两百行素材每帧 stat 两百次就够了。状态在打开面板、导入完成时算一次缓存起来。

mod actions;
mod covers;
mod drop;
mod import;
mod panel;
mod render;
mod tags;
#[cfg(test)]
mod tests;

pub(crate) use drop::{AssetDrag, AssetDrop, lane_offset};
pub(crate) use panel::{LibraryImport, LibraryPanel};
