//! 浮层回退与提示：Esc 的退层顺序、屏幕上的临时提示。

use super::super::*;

impl PlayerApp {
    /// Esc 的语义：一层一层往回退，而不是直接全关。
    pub(in crate::app) fn exit_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 全屏里右侧面板与菜单栏都不画（见 `render`）：它们的关闭分支要是还排在
        // 队伍里，Esc 第一下关掉的就是一个看不见的东西，屏幕上毫无变化——看着像
        // Esc 失灵，退出全屏得按第二下。所以全屏时跳过那几层。
        let fullscreen = window.is_fullscreen();
        if self.playlist_import.is_some() {
            // 选择器是模态，Esc 先关它，别顺手把下面的面板也收掉。
            self.close_playlist_import_dialog(cx);
        } else if self.tab_rename.is_some() {
            // 输入框自己会吃掉 Esc（见 `on_rename_key`），这里兜住「焦点不在
            // 根视图」的那一次，保证 Esc 总能关掉它。
            self.tab_rename = None;
        } else if !fullscreen && self.clip.is_some() {
            // 输入框有焦点时 Esc 被 `on_clip_key` 截走（只退出焦点）；这里处理
            // 「焦点已经在播放器上」的那一次，把面板收起来。
            self.close_clip_dialog(cx);
        } else if self
            .convert
            .as_ref()
            .is_some_and(|dialog| dialog.picker_open())
        {
            // 格式下拉展开时 Esc 先收列表：面板本身还留着。
            // 下拉里的搜索框会先吃掉这次按键，这里兜住「焦点不在根视图」的那一次。
            self.convert_close_picker(cx);
        } else if !fullscreen && self.convert.is_some() {
            // 转换面板和裁切占同一条通道，两者不会同时开着。
            self.close_convert_dialog(cx);
        } else if self.show_settings {
            self.show_settings = false;
        } else if self.file_info.is_some() {
            self.file_info = None;
        } else if self.timeline_menu.is_some() {
            // 时间线只在工程模式下画，全屏时它跟右侧面板一样不存在，不必再判。
            self.timeline_menu = None;
        } else if !fullscreen && self.item_menu.is_some() {
            self.item_menu = None;
        } else if !fullscreen && self.tab_menu.is_some() {
            self.tab_menu = None;
        } else if !fullscreen && self.panel_menu.is_some() {
            self.panel_menu = None;
        } else if !fullscreen && self.menu_bar.is_some() {
            self.menu_bar = None;
        } else if self.menu == Some(Menu::SpeedCustom) {
            // 自定义面板是倍速菜单的第二层，Esc 先退回预设列表。
            self.menu = Some(Menu::Speed);
        } else if self.menu.is_some() {
            self.menu = None;
        } else if self.screenshot_card.is_some() {
            // 卡片是最后一个浮上来的东西，Esc 先收它，再考虑关面板、退全屏。
            self.clear_screenshot_card();
        } else if self.mini.is_some() {
            // 小窗是「从主窗口分出去」的，Esc 表示收回来。
            self.dismiss_mini(cx);
        } else if !fullscreen && self.show_history {
            // 侧栏不是模态，Esc 排在菜单、对话框之后：先收上面的浮层。
            self.close_history_panel(cx);
        } else if !fullscreen && self.settings.show_playlist {
            self.set_playlist_visible(false);
        } else if fullscreen {
            window.toggle_fullscreen();
        }
        self.poke();
        cx.notify();
    }

    pub(in crate::app) fn flash(&mut self, message: impl Into<SharedString>) {
        let text = message.into();
        self.osd = Some((
            text.split('\n').map(|line| line.to_string()).collect(),
            Instant::now(),
        ));
    }
}
