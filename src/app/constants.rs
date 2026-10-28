//! 布局尺寸、交互步长与超时时间。

use super::*;

pub(super) const SEEK_STEP: f64 = 5.0;
pub(super) const VOLUME_STEP: f64 = 5.0;
/// 倍速菜单里的固定档位。自定义滑块的量程比它宽，两边共用同一套显示格式。
pub(super) const SPEED_PRESETS: [f64; 9] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0];
/// 自定义倍速滑块的量程，与面板上标出的 0.1–5.0 一致。
pub(super) const SPEED_MIN: f64 = 0.1;
pub(super) const SPEED_MAX: f64 = 5.0;
/// 滑块的最小步进：这个宽度下拖到 0.01 也选不准，0.05 已经够用且数字好读。
pub(super) const SPEED_STEP: f64 = 0.05;
/// 触控板走的是像素增量，攒够这么远算「一格」，走一帧。
pub(super) const FRAME_SCROLL_PX: f32 = 48.0;
pub(super) const CONTROLS_IDLE: Duration = Duration::from_millis(2600);
pub(super) const OSD_LIFETIME: Duration = Duration::from_millis(900);
/// 截图卡片的宽度与预览区高度。
pub(super) const SCREENSHOT_CARD_WIDTH: f32 = 280.0;
pub(super) const SCREENSHOT_PREVIEW_HEIGHT: f32 = 158.0;
/// 没人搭理时多久自动收起。鼠标停在卡片上时不计时。
pub(super) const SCREENSHOT_CARD_LIFETIME: Duration = Duration::from_secs(10);
pub(super) const MENUBAR_HEIGHT: f32 = 32.0;
/// 播放列表面板拖动时的宽度边界。视频那边至少留 480px：再窄控制条上的
/// 倍速/循环/全屏就挤成一团，拖到头也不该出现这种布局。
pub(super) const PLAYLIST_MIN_WIDTH: f32 = 220.0;
pub(super) const PLAYLIST_MAX_WIDTH: f32 = 760.0;
pub(super) const VIDEO_MIN_WIDTH: f32 = 480.0;
pub(super) const PLAYLIST_RESIZER_WIDTH: f32 = 6.0;
/// 行右键菜单的尺寸，只用来把弹窗往窗口里收。
pub(super) const ITEM_MENU_WIDTH: f32 = 168.0;
pub(super) const ITEM_MENU_HEIGHT: f32 = 118.0;

/// 小窗宽度的上下限：太窄看不清，太宽就失去「小」的意义了。
pub(super) const MINI_MIN_WIDTH: f32 = 320.0;
pub(super) const MINI_MAX_WIDTH: f32 = 560.0;
/// 小窗离屏幕边缘的距离。底部额外多留一段，给任务栏 / Dock 让位。
pub(super) const MINI_MARGIN: f32 = 24.0;
pub(super) const MINI_BOTTOM_MARGIN: f32 = 72.0;
