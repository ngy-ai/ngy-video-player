//! 文件关联的公共接口。
//!
//! 这里只定义「能做什么」，怎么做在各自平台模块里。三个平台的行为必须一致：
//! 注册可重复执行、撤销只删自己写的东西、从不试图替用户决定默认播放器
//! （见 `docs/adr/0002` 与 `CONTEXT.md` 的「已注册关联」「默认播放器」）。

use std::path::PathBuf;

use anyhow::Result;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

#[cfg(not(windows))]
mod unsupported;
#[cfg(not(windows))]
use unsupported as imp;

/// 关联在系统里的落点。
#[derive(Clone, Debug, Default)]
pub struct Status {
    /// 系统里登记了本程序的扩展名个数；0 表示没注册过。
    pub registered: usize,
    /// 注册项里指向的可执行文件。和当前进程不一致，说明关联已经过期。
    pub target: Option<PathBuf>,
}

impl Status {
    pub fn is_registered(&self) -> bool {
        self.registered > 0
    }
}

/// 当前平台是否支持自动注册。
pub fn supported() -> bool {
    imp::SUPPORTED
}

/// 幂等注册：已经注册过就按当前 exe 的路径重写一遍。
///
/// 只让本程序进入候选列表，不会改变用户已经选定的默认播放器。
pub fn register() -> Result<Status> {
    imp::register()
}

/// 撤销：只删本程序写下的键，不碰系统里指向别的应用的任何设置。
pub fn remove() -> Result<()> {
    imp::remove()
}

/// 只读查询，供菜单展示。任何一步失败都当作「没注册」，展示层不该因此报错。
pub fn status() -> Status {
    imp::status().unwrap_or_default()
}

/// 打开系统的「默认应用」界面。用户必须在那里亲手选定——这不是实现偷懒，
/// 而是 Windows 8 之后唯一的合法路径。
pub fn open_system_settings() -> Result<()> {
    imp::open_system_settings()
}

/// 期望注册的扩展名个数。
pub fn count() -> usize {
    crate::formats::associable_extensions().len()
}
