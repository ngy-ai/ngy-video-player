//! 平台集成。
//!
//! 这里处理三件系统层面的事：把本程序登记成系统眼中「可以当视频播放器」的候选、
//! 在文件管理器里定位文件、以及给窗口打开/关闭置顶。三个平台的机制互不相通
//! （注册表 / `.desktop` / LaunchServices，TOPMOST / NSWindow level / `_NET_WM_STATE`），
//! 所以按 `cfg` 分派、各自原生实现，不引入跨平台 crate。理由与后果见 `docs/adr/0001`。

pub mod associations;
pub mod clipboard;
pub mod open;
pub mod reveal;
pub mod topmost;
