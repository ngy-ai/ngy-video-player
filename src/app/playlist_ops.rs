//! 播放列表操作：打开文件/目录、标签页、选中与删除、拖放。
//!
//! 实现按主题分在子模块里：`open` 管文件对话框与「打开 / 添加」，`tabs` 管页签，
//! `edit` 管列表编辑与行菜单，`play` 管播放推进，`overlay` 管 Esc 退层与提示。
//! 都是同一个 `PlayerApp` 的 impl 块，共享字段仍由 `app.rs` 私有持有。

mod edit;
mod open;
mod overlay;
mod play;
mod tabs;
