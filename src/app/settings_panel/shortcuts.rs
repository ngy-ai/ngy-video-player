//! 设置窗口 ·「快捷键」页：键位表。
//!
//! 原来是一个独立的「键盘快捷键」浮层，`?` 打开。并进设置窗口之后 `?` 仍然直达，
//! 但不用再维护第二套浮层布局。

use super::super::*;

impl PlayerApp {
    pub(super) fn shortcuts_page(&self) -> AnyElement {
        // 按键位的作用分组：一长串二十行不分组，找「删除」和找「全屏」一样费劲。
        let play = [
            ("空格", "播放 / 暂停"),
            ("← →", "快退 / 快进 5 秒"),
            ("↑ ↓", "音量"),
            ("滚轮", "音量"),
            ("Ctrl/⌘ 滚轮", "逐帧：上滚后退 / 下滚前进"),
            ("Ctrl/⌘ ← →", "逐帧后退 / 前进"),
            ("N / P", "下一个 / 上一个"),
            ("[ ]", "减速 / 加速"),
            ("M", "静音"),
            ("Z / X", "字幕提前 / 延后 0.1 秒"),
        ];
        let view = [
            ("F / 双击", "全屏"),
            ("T", "小窗"),
            ("L", "播放列表"),
            ("Ctrl/⌘ Shift E", "工程素材库"),
            ("A", "设置（画面页）"),
            ("S", "截图"),
            ("I", "播放统计（帧率 / 丢帧）"),
            ("O", "打开文件"),
            ("?", "设置（本页）"),
            ("Esc", "关闭浮层 / 退出全屏"),
        ];
        let list = [
            ("Ctrl/⌘ 点击", "列表多选"),
            ("Shift 点击", "连选一段"),
            ("右键列表项", "删除 / 定位文件 / 查看格式"),
            ("Delete", "删除所选项"),
        ];
        // 工程模式的键位单独一组：它们只在时间线上有效，混进「界面」里会看不出来。
        let timeline = [
            ("Ctrl/⌘ Z", "撤销"),
            ("Ctrl/⌘ Shift Z", "重做"),
            ("Ctrl/⌘ K", "在播放头处切割"),
            ("Delete", "删除片段（留空隙）"),
            ("Shift+Delete", "波纹删除（后面补齐）"),
            ("Shift I / Shift O", "时间线入点 / 出点"),
            ("Alt I / Alt O", "源入点 / 出点（试听原片时）"),
            (", / .", "把试听的原片插入 / 覆盖到播放头"),
            ("Shift M", "标记"),
            ("J / K / L", "倒放 / 停 / 正放（连按 1×→2×→4×）"),
            ("Ctrl/⌘ G", "跳到时间码"),
            ("- / = / 0", "缩小 / 放大 / 整条可见"),
        ];

        div()
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(shortcut_group("播放", &play))
            .child(shortcut_group("界面", &view))
            .child(shortcut_group("播放列表", &list))
            .child(shortcut_group("时间线", &timeline))
            .into_any_element()
    }
}

/// 一组快捷键：小标题 + 若干「键位 + 说明」两列行。
fn shortcut_group(title: &'static str, rows: &[(&'static str, &'static str)]) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .pb(px(4.0))
                .text_size(px(12.0))
                .text_color(text_muted())
                .child(title),
        )
        .children(rows.iter().map(|(keys, action)| {
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .h(px(26.0))
                // 键位列给固定宽度，右边的说明才能上下对齐成一列。
                .child(
                    div()
                        .w(px(112.0))
                        .flex_none()
                        .text_size(px(12.0))
                        .text_color(text_muted())
                        .child(*keys),
                )
                .child(div().text_size(px(13.0)).text_color(text()).child(*action))
        }))
        .into_any_element()
}
