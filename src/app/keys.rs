//! 键位表：动作 → 键位串，一处集中，方便对照与改动。
//!
//! 键位串按键帽字符匹配：`comma` / `period` 这类名字在 GPUI 里不存在，`,` / `.`
//! 要写成字符本身；`?` 那种带修饰的得写成 `shift-/`。

use super::*;

pub fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("space", TogglePlay, None),
        KeyBinding::new("right", SeekForward, None),
        KeyBinding::new("left", SeekBackward, None),
        // Ctrl/⌘ + ←/→：逐帧。macOS 上 Ctrl 是系统级修饰键（切换桌面），
        // 用 ⌘；其它平台用 Ctrl。
        KeyBinding::new(
            if cfg!(target_os = "macos") {
                "cmd-left"
            } else {
                "ctrl-left"
            },
            StepBackward,
            None,
        ),
        KeyBinding::new(
            if cfg!(target_os = "macos") {
                "cmd-right"
            } else {
                "ctrl-right"
            },
            StepForward,
            None,
        ),
        KeyBinding::new("up", VolumeUp, None),
        KeyBinding::new("down", VolumeDown, None),
        KeyBinding::new("m", ToggleMute, None),
        KeyBinding::new("f", ToggleFullscreen, None),
        KeyBinding::new("t", ToggleMini, None),
        KeyBinding::new("a", OpenPictureSettings, None),
        KeyBinding::new("l", TogglePlaylist, None),
        // J/K/L：工程模式里 `l` 交给「正向变速」（播放列表在工程模式下不出现），
        // `j` / `k` 专给变速，播放器模式下按下去什么也不做。
        KeyBinding::new("j", ShuttleBackward, None),
        KeyBinding::new("k", ShuttleStop, None),
        // 跳到时间码：与常见 NLE 的「按时间码定位」同一颗键。
        KeyBinding::new(
            if cfg!(target_os = "macos") {
                "cmd-g"
            } else {
                "ctrl-g"
            },
            JumpToTimecode,
            None,
        ),
        // 工程模式的素材库与播放列表不是一回事（前者是工程素材，后者是播放列表），
        // 键位错开：Ctrl/⌘ + Shift + E，与常见剪辑软件的「项目素材」位置一致。
        KeyBinding::new(
            if cfg!(target_os = "macos") {
                "cmd-shift-e"
            } else {
                "ctrl-shift-e"
            },
            ToggleLibrary,
            None,
        ),
        KeyBinding::new("o", OpenFile, None),
        // 保存沿用各平台的习惯键位；macOS 上 Ctrl 是系统修饰键，只能用 ⌘。
        KeyBinding::new(
            if cfg!(target_os = "macos") {
                "cmd-s"
            } else {
                "ctrl-s"
            },
            SaveProject,
            None,
        ),
        KeyBinding::new(
            if cfg!(target_os = "macos") {
                "cmd-shift-s"
            } else {
                "ctrl-shift-s"
            },
            SaveProjectAs,
            None,
        ),
        // 与 Premiere 一致：Ctrl/⌘ + K 在播放头处切开。
        KeyBinding::new(
            if cfg!(target_os = "macos") {
                "cmd-k"
            } else {
                "ctrl-k"
            },
            SplitPlayhead,
            None,
        ),
        // `m` 已经是静音，标记只能用带 Shift 的那个。
        KeyBinding::new("shift-m", ToggleMarker, None),
        // 入出点同理：`i` 是统计、`o` 是打开文件，时间线的标注统一放在 Shift 组。
        KeyBinding::new("shift-i", SetInPoint, None),
        KeyBinding::new("shift-o", SetOutPoint, None),
        // 时间线缩放：`-` / `=` 各一级，`0` 回到「整条序列可见」。
        KeyBinding::new("-", ZoomTimelineOut, None),
        KeyBinding::new("=", ZoomTimelineIn, None),
        KeyBinding::new("0", FitTimeline, None),
        // 撤销 / 重做：各平台都用习惯键位。
        KeyBinding::new(
            if cfg!(target_os = "macos") {
                "cmd-z"
            } else {
                "ctrl-z"
            },
            UndoEdit,
            None,
        ),
        KeyBinding::new(
            if cfg!(target_os = "macos") {
                "cmd-shift-z"
            } else {
                "ctrl-shift-z"
            },
            RedoEdit,
            None,
        ),
        KeyBinding::new("n", NextFile, None),
        KeyBinding::new("p", PreviousFile, None),
        KeyBinding::new("[", SpeedDown, None),
        KeyBinding::new("]", SpeedUp, None),
        // 与 mpv 一致：Z 提前、X 延后，一次 0.1 秒。
        KeyBinding::new("z", SubtitleEarlier, None),
        KeyBinding::new("x", SubtitleLater, None),
        KeyBinding::new("i", ToggleStats, None),
        KeyBinding::new("s", TakeScreenshot, None),
        KeyBinding::new("escape", ExitOverlay, None),
        KeyBinding::new("delete", DeleteSelected, None),
        // Mac 键盘上的退格键发的是 backspace，和 delete 不是同一个键。
        KeyBinding::new("backspace", DeleteSelected, None),
        // 按住 Shift 删除是波纹删除：后面的片段往前补，不留空隙。
        KeyBinding::new("shift-delete", RippleDeleteSelected, None),
        KeyBinding::new("shift-backspace", RippleDeleteSelected, None),
        // 三点编辑：`i` / `o` / `shift-i` / `shift-o` 都已经被占了（统计、打开文件、
        // 时间线入出点），源入出点用 Alt 组；插入与覆盖用 Premiere 的 `,` / `.`。
        KeyBinding::new("alt-i", SetSourceIn, None),
        KeyBinding::new("alt-o", SetSourceOut, None),
        // 键位串按键帽字符匹配：写成 `comma` / `period` 永远匹配不上。
        KeyBinding::new(",", InsertFromSource, None),
        KeyBinding::new(".", OverwriteFromSource, None),
        // `?` 不在这里绑定：键位串按键帽字符匹配，`?` 得写成 `shift-/`，
        // 且随键盘布局变化。改由视图的 key_down 按实际字符处理。
    ]
}
