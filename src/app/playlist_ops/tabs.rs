//! 页签：新建、切换、关闭、右键菜单与改名。

use super::super::*;

impl PlayerApp {
    // -- 多列表 ------------------------------------------------------------

    pub(in crate::app) fn add_tab(&mut self, cx: &mut Context<Self>) {
        let name = self.playlists.auto_name();
        self.playlists.add(name);
        self.selection.clear();
        self.selection_anchor = None;
        self.playlists.save();
        self.poke();
        cx.notify();
    }

    /// 切页不动播放：面板同时兼任「浏览」的职责，切过去就换歌太打扰。
    pub(in crate::app) fn select_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        self.playlists.set_active(index);
        self.selection.clear();
        self.selection_anchor = None;
        self.playlists.save();
        self.poke();
        cx.notify();
    }

    pub(in crate::app) fn close_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        let name = self
            .playlists
            .tabs()
            .get(index)
            .map(|tab| tab.name.clone())
            .unwrap_or_default();
        // 默认页不画关闭按钮，菜单里那两项也是灰的；这里是最后一道闸，
        // 少一条路绕过「不能删」的约定。
        if self.playlists.is_default(index) {
            self.flash("「默认」列表不能删除");
            cx.notify();
            return;
        }
        if !self.playlists.remove(index) {
            return;
        }
        // 关页面不动播放：正在放的东西还在引擎里，让它自然放完，只是不再有「下一集」。
        if self.playing_tab == Some(index) {
            self.playing_tab = None;
        } else if let Some(playing) = self.playing_tab
            && playing > index
        {
            self.playing_tab = Some(playing - 1);
        }
        self.selection.clear();
        self.selection_anchor = None;
        self.playlists.save();
        self.flash(format!("已关闭列表「{name}」"));
        self.poke();
        cx.notify();
    }

    /// 页签右键：在鼠标位置弹出菜单。越界就往窗口里收，别飘到屏幕外。
    pub(in crate::app) fn open_tab_menu(
        &mut self,
        index: usize,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.playlists.len() {
            return;
        }
        let viewport = window.viewport_size();
        let max_x = (f32::from(viewport.width) - TAB_MENU_WIDTH - 8.0).max(8.0);
        let max_y = (f32::from(viewport.height) - TAB_MENU_HEIGHT - 8.0).max(8.0);
        let position = point(
            px(f32::from(position.x).clamp(8.0, max_x)),
            px(f32::from(position.y).clamp(8.0, max_y)),
        );

        self.tab_menu = Some(TabMenu { index, position });
        // 同一时刻只留一层右键菜单。
        self.item_menu = None;
        self.panel_menu = None;
        self.menu = None;
        self.menu_bar = None;
        self.poke();
        cx.notify();
    }

    /// 打开重命名输入框，预填当前名字。
    pub(in crate::app) fn begin_rename(&mut self, index: usize, cx: &mut Context<Self>) {
        self.tab_menu = None;
        // 默认列表的名字是程序的一部分（打开文件的结果放在这一页），不给改。
        if self.playlists.is_default(index) {
            self.flash("「默认」列表的名字固定，不能改名");
            cx.notify();
            return;
        }
        let Some(tab) = self.playlists.tabs().get(index) else {
            return;
        };
        let text = tab.name.clone();
        self.tab_rename = Some(TabRename {
            index,
            text,
            typed: false,
        });
        self.poke();
        cx.notify();
    }

    /// 确认改名。名字为空时保留原名，等于什么也没发生。
    pub(in crate::app) fn finish_rename(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.tab_rename.take() else {
            return;
        };
        let name = dialog.text.trim().to_string();
        if name.is_empty() {
            // 空名字会让页签看起来像丢了标题；退回原名，比留个空白页签好。
            self.flash("列表名不能为空，已保留原名");
            cx.notify();
            return;
        }
        let Some(current) = self
            .playlists
            .tabs()
            .get(dialog.index)
            .map(|tab| tab.name.clone())
        else {
            // 页签在输入过程中被关掉了：没有可改的对象，静默收场。
            cx.notify();
            return;
        };
        if current == name {
            cx.notify();
            return;
        }
        if !self.playlists.rename(dialog.index, name.clone()) {
            // 只剩两种原因：改的是默认列表，或者想给别的页起「默认」这个名字。
            self.flash("「默认」是保留名，不能改也不能重名");
            cx.notify();
            return;
        }
        self.playlists.save();
        self.flash(format!("已改名为「{name}」"));
        self.poke();
        cx.notify();
    }

    /// 重命名输入框的键盘：Enter 确认、Esc 取消、退格删除、其余可打印字符追加。
    pub(in crate::app) fn on_rename_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.tab_rename.is_none() {
            return false;
        }
        let modifiers = event.keystroke.modifiers;
        // 粘贴：输入框走的是按键转换，中文字符进不来（没有输入法处理器），
        // 想用中文名只能从别处复制进来。
        if (modifiers.control || modifiers.platform) && event.keystroke.key == "v" {
            if let Some(text) = cx
                .read_from_clipboard()
                .and_then(|item| item.text().map(|text| text.to_string()))
            {
                if let Some(dialog) = self.tab_rename.as_mut() {
                    let clipped: String = text
                        .chars()
                        .filter(|ch| !ch.is_control())
                        .take(MAX_TAB_NAME_CHARS)
                        .collect();
                    dialog.text = clipped.trim().to_string();
                    dialog.typed = true;
                }
                cx.notify();
            }
            return true;
        }
        // 其余组合键留给播放器：输入框只吃纯文本和编辑键。
        if modifiers.control || modifiers.platform || modifiers.alt {
            return false;
        }

        match event.keystroke.key.as_str() {
            "enter" => {
                self.finish_rename(cx);
                true
            }
            "escape" => {
                self.tab_rename = None;
                cx.notify();
                true
            }
            "backspace" => {
                if let Some(dialog) = self.tab_rename.as_mut() {
                    // 退格也算「动过键盘」：之后不能再把预填的名字整个清掉。
                    dialog.typed = true;
                    dialog.text.pop();
                }
                cx.notify();
                true
            }
            _ => {
                let Some(typed) = event.keystroke.key_char.as_deref() else {
                    // 方向键、功能键这些没有字符的按键：对话框开着时一律吞掉。
                    // 漏下去会让方向键去 seek，用户以为自己只是在挪光标。
                    return true;
                };
                if typed.is_empty() || typed.chars().any(|ch| ch.is_control()) {
                    return false;
                }
                if let Some(dialog) = self.tab_rename.as_mut() {
                    // 第一次敲字就整个替换：预填只是告诉用户正在改哪一页。
                    if !dialog.typed {
                        dialog.text.clear();
                        dialog.typed = true;
                    }
                    if dialog.text.chars().count() < MAX_TAB_NAME_CHARS {
                        dialog.text.push_str(typed);
                    }
                }
                cx.notify();
                true
            }
        }
    }
}
