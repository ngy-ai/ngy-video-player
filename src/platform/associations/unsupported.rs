//! 尚未实现文件关联的平台。
//!
//! 留一个接口齐全的模块而不是到处写 `#[cfg]`，是为了让菜单和状态展示不必关心平台：
//! `supported()` 为假时上层只会把相关条目置灰，而不是编译不过。

use anyhow::{Result, bail};

use super::Status;

pub const SUPPORTED: bool = false;

pub fn register() -> Result<Status> {
    bail!("{} 上还没有实现文件关联注册", std::env::consts::OS)
}

pub fn remove() -> Result<()> {
    bail!("{} 上还没有实现文件关联注册", std::env::consts::OS)
}

pub fn status() -> Result<Status> {
    Ok(Status::default())
}

pub fn open_system_settings() -> Result<()> {
    bail!("当前平台没有统一的默认应用设置页")
}

pub fn guidance() -> &'static str {
    "当前平台还不支持自动关联，需要在系统设置里手动指定。"
}
