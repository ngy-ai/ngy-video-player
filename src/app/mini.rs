//! 迷你播放器：独立小窗、悬停控制条与拖动定位。
//!
//! 拆成四个子模块：`player` 是实体与窗口/状态逻辑，`bar` 是顶部悬浮条，
//! `controls` 是底部控制栏与音量面板，`render` 是整窗组装。尺寸与指针坐标
//! 换算这类纯计算单独放 `geometry`，不碰 `Context`，测试可以直接调。

use super::*;

mod bar;
mod controls;
mod geometry;
mod menu;
mod player;
mod render;

#[cfg(test)]
mod tests;

pub(super) use geometry::{mini_aspect, mini_width, origin_for_ratio};
pub(super) use player::MiniPlayer;
