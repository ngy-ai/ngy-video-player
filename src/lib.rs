//! 跨平台视频播放器：GPUI 负责界面，libmpv 负责解码。

// 日志宏要在其它模块之前引入（`macro_rules!` 按文本顺序生效）。
#[macro_use]
pub mod logging;

pub mod app;
pub mod formats;
pub mod platform;
pub mod player;
pub mod playlist;
pub mod settings;
pub mod ui;

pub use player::Engine;
