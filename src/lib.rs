//! 跨平台视频播放器：GPUI 负责界面，libmpv 负责解码。

// 日志宏要在其它模块之前引入（`macro_rules!` 按文本顺序生效）。
#[macro_use]
pub mod logging;

pub mod app;
pub mod clip;
pub mod convert;
// 内嵌资源（libmpv / ffmpeg）的解压与缓存：对外没有 API，只是两个模块共用的落盘逻辑。
mod embed;
// 硬件编码器探测：裁切与格式转换共用，对外没有 API。
mod encoder;
pub mod formats;
pub mod media;
pub mod media_cache;
pub mod metadata;
pub mod multicam;
pub mod platform;
pub mod player;
pub mod playlist;
pub mod proxy;
pub mod remux;
pub mod scenes;
pub mod scope;
pub mod screenshot;
pub mod settings;
// 字幕编码判断：轨道编辑与元数据编辑共用，不对外暴露。
mod subtitle_codec;
pub mod timeline;
pub mod ui;

pub use player::Engine;
