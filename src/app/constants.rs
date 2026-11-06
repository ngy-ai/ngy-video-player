//! 布局尺寸、交互步长与超时时间。

use super::*;

/// 已经发出换片命令、但引擎还没下发新画面代次时的哨兵。
/// 这期间到达的帧都属于上一部片子，界面一概不认。
pub(super) const FRAME_EPOCH_PENDING: u64 = u64::MAX;
pub(super) const SEEK_STEP: f64 = 5.0;
pub(super) const VOLUME_STEP: f64 = 5.0;
/// 倍速菜单里的固定档位。自定义滑块的量程比它宽，两边共用同一套显示格式。
pub(super) const SPEED_PRESETS: [f64; 9] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0];
/// 自定义倍速滑块的量程，与面板上标出的 0.1–5.0 一致。
///
/// 定义在 `settings` 里：配置读入时也要按同一个范围校正，界面层不能有另一份。
pub(super) use crate::settings::{SPEED_MAX, SPEED_MIN};
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
/// 工程模式的时间线：标题条、标尺、轨道行高与左侧名称列。
pub(super) const TIMELINE_HEADER: f32 = 30.0;
/// 标尺：比原来高 6 像素，帧刻度铺满时还能看清层次。
pub(super) const TIMELINE_RULER: f32 = 32.0;
/// 一帧占到这个宽度就把刻度铺到帧；再窄就只剩一片糊。
pub(super) const FRAME_TICK_PX: f32 = 4.0;
pub(super) const TIMELINE_GUTTER: f32 = 108.0;
pub(super) const TIMELINE_TRACK_HEIGHT: f32 = 46.0;
pub(super) const TIMELINE_TRACK_GAP: f32 = 2.0;
/// 时间线高度的上下限：太矮放不下标尺与一条轨道，太高会把预览挤没。
pub(super) const TIMELINE_MIN_HEIGHT: f32 = 170.0;
pub(super) const TIMELINE_MAX_HEIGHT: f32 = 320.0;
/// 播放列表面板拖动时的宽度边界。上限还要再受 `CONTROLS_MIN_WIDTH` 约束，
/// 见 `PlayerApp::playlist_max_width`。
pub(super) const PLAYLIST_MIN_WIDTH: f32 = 220.0;
pub(super) const PLAYLIST_MAX_WIDTH: f32 = 760.0;
/// 裁切面板的宽度。它和播放列表一样贴右侧，视频与控制条按它让位。
///
/// 固定宽度而不给拖动条：裁切面板的内容都是定宽控件，拖宽只会让中间空出一片。
pub(super) const CLIP_PANEL_WIDTH: f32 = 340.0;
/// 格式转换面板的宽度。比裁切宽一点：格式卡片两列并排，太窄小字会折成两行。
pub(super) const CONVERT_PANEL_WIDTH: f32 = 360.0;
/// 工程模式编辑历史侧栏的宽度。一行只放序号、操作说明与状态，不必跟裁切一样宽。
pub(super) const HISTORY_PANEL_WIDTH: f32 = 260.0;
/// 工程模式左侧素材库的宽度。一行放封面 + 名字 + 规格，比历史侧栏略宽。
pub(super) const LIBRARY_PANEL_WIDTH: f32 = 280.0;
/// 控制条一行排下全部按钮所需的宽度。
///
/// 控制条按「窗口宽 - 面板宽」定宽，面板一旦把这一条压窄，右侧的倍速、
/// 循环、全屏就会越界画到播放列表上去。所以面板能拖到多宽，得先扣掉这个数。
/// 加减控制条上的按钮或改它们的尺寸时，这里要跟着改。
pub(super) const CONTROLS_MIN_WIDTH: f32 = 18.0 * 2.0 // 左右内边距
    + 34.0 * 8.0 // 上一集、播放、下一集、字幕、音轨、列表、迷你、全屏
    + 6.0 // 播放键与音量之间的空档
    + (34.0 + 2.0 + 84.0) // 音量图标 + 间隙 + 滑条
    + (8.0 + 104.0) // 时间文字（含左边距），按 "10:29:00 / 10:29:00" 算
    + 64.0 // 倍速 pill
    + 56.0 // 循环 pill
    + 6.0 * 13.0; // 行内间距
pub(super) const PLAYLIST_RESIZER_WIDTH: f32 = 6.0;
/// 树形列表每深一层的缩进。太窄看不出层级，太宽会把长文件名挤没。
pub(super) const PLAYLIST_INDENT: f32 = 14.0;
/// 行右键菜单的尺寸，只用来把弹窗往窗口里收。
pub(super) const ITEM_MENU_WIDTH: f32 = 168.0;
pub(super) const ITEM_MENU_HEIGHT: f32 = 118.0;
/// 右键菜单里一行的高度：`widgets::item_menu_entry` 的上下各 7 像素内边距，
/// 加 13 号字一行。改那边的内边距或字号时要跟着改。
/// 用途只有一个：估算菜单总高，判断在鼠标上方还是下方展开。
pub(super) const ITEM_MENU_ROW_HEIGHT: f32 = 33.0;
/// 菜单外框的上下内边距（`py(6.0)`）。
pub(super) const ITEM_MENU_PADDING: f32 = 6.0;
/// 菜单里的分隔线：上下各 4 像素外边距加 1 像素线。
pub(super) const ITEM_MENU_SEPARATOR_HEIGHT: f32 = 9.0;
/// 页签右键菜单的尺寸，同样只用来把弹窗往窗口里收。
pub(super) const TAB_MENU_WIDTH: f32 = 152.0;
pub(super) const TAB_MENU_HEIGHT: f32 = 82.0;
/// 列表名的长度上限。页签只显示 140 像素，再长也只能省略，不如早点截断。
pub(super) const MAX_TAB_NAME_CHARS: usize = 40;

/// 小窗宽度的上下限：太窄看不清，太宽就失去「小」的意义了。
pub(super) const MINI_MIN_WIDTH: f32 = 320.0;
pub(super) const MINI_MAX_WIDTH: f32 = 560.0;
/// 小窗离屏幕边缘的距离。底部额外多留一段，给任务栏 / Dock 让位。
pub(super) const MINI_MARGIN: f32 = 24.0;
pub(super) const MINI_BOTTOM_MARGIN: f32 = 72.0;
