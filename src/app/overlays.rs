//! 浮层：OSD、截图卡片、错误提示、条目菜单、文件信息。
//!
//! 全部是「贴在画面上的东西」，按职责分文件：`hud` 是播放统计与 OSD，
//! `cards` 是截图卡片与错误条，`menus` 是条目 / 页签的弹出菜单，`file_info`
//! 是文件信息面板。方法都挂在 `PlayerApp` 上，入口只声明模块。

mod cards;
mod file_info;
mod hud;
mod menus;
