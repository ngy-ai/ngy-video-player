//! 平台集成。
//!
//! 目前只有一件事：把本程序登记成系统眼中「可以当视频播放器」的候选。三个平台的
//! 机制互不相通（注册表 / `.desktop` / LaunchServices），所以按 `cfg` 分派、各自原生
//! 实现，不引入跨平台 crate。理由与后果见 `docs/adr/0001`。

pub mod associations;
pub mod reveal;
