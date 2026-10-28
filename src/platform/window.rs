//! 把窗口提到前台。
//!
//! gpui 的 `Window::activate_window()` 在 Windows 上只做 `ShowWindowAsync(SW_RESTORE)`
//! ＋ `SetActiveWindow` ＋ `SetFocus`。前一个是异步的，后两个是**线程内**的概念：
//! 它们能把窗口从最小化状态恢复，却不会把窗口抬到别的进程之上。于是「从小窗还原
//! 主窗口」就会还原成功但停在别的应用后面，看起来像没反应。
//!
//! 这里补上真正管用的那两步：`BringWindowToTop` 改 Z 序、`SetForegroundWindow` 抢前台。
//! 后者受 Windows 的前台锁限制（只有当前前台进程才能调用成功），而我们的调用点恰好
//! 都发生在用户刚点过本程序的窗口之后，此时进程就是前台，能过。

use gpui_kit::Window;

/// 还原并置前。返回 `false` 表示这个平台没有对应实现，调用方退回
/// `Window::activate_window()`。
pub fn bring_to_front(window: &Window) -> bool {
    imp::bring_to_front(window)
}

/// 切换窗口在任务栏 / Alt+Tab 里是否出现。
///
/// 小窗是工具窗口（`WS_EX_TOOLWINDOW`），本来没有任务栏入口——这是为了让
/// 「任务栏里本程序只有一条」成立。可一旦用户取消置顶，它就可能被别的窗口
/// 完全盖住，而工具窗口既不在任务栏也不在 Alt+Tab 里，等于彻底找不回来。
/// 所以取消置顶时要把它换成普通窗口，重新置顶再换回去。
///
/// 返回 `false` 表示这个平台不支持，调用方不用管。
pub fn set_taskbar_visible(window: &Window, visible: bool) -> bool {
    imp::set_taskbar_visible(window, visible)
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    use gpui_kit::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    const SW_RESTORE: i32 = 9;
    const SW_HIDE: i32 = 0;
    const SW_SHOWNOACTIVATE: i32 = 4;

    const GWL_EXSTYLE: i32 = -20;
    const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
    const WS_EX_APPWINDOW: isize = 0x0004_0000;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn ShowWindow(hwnd: *mut c_void, cmd: i32) -> i32;
        fn BringWindowToTop(hwnd: *mut c_void) -> i32;
        fn SetForegroundWindow(hwnd: *mut c_void) -> i32;
        fn SetActiveWindow(hwnd: *mut c_void) -> *mut c_void;
        fn GetWindowLongPtrW(hwnd: *mut c_void, index: i32) -> isize;
        fn SetWindowLongPtrW(hwnd: *mut c_void, index: i32, value: isize) -> isize;
    }

    /// 取 HWND。注意不能用 `window.window_handle()`：那是 GPUI 自己的固有方法，
    /// 返回的是 `AnyWindowHandle`，原生句柄要走 trait 上的同名方法。
    fn hwnd(window: &Window) -> Option<*mut c_void> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        match handle.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as *mut c_void),
            _ => None,
        }
    }

    pub fn bring_to_front(window: &Window) -> bool {
        let Some(hwnd) = hwnd(window) else {
            return false;
        };
        unsafe {
            // 顺序有讲究：最小化的窗口不先还原，后面几步作用的都是一个不可见的
            // 窗口；先还原再改 Z 序，最后才是抢前台。
            ShowWindow(hwnd, SW_RESTORE);
            BringWindowToTop(hwnd);
            SetForegroundWindow(hwnd);
            SetActiveWindow(hwnd);
        }
        // 抢前台失败通常只是前台锁在拦，不代表平台不支持；此时窗口至少已经还原
        // 并且抬到了 Z 序顶端，不该再让调用方去走另一条路径。
        true
    }

    pub fn set_taskbar_visible(window: &Window, visible: bool) -> bool {
        let Some(hwnd) = hwnd(window) else {
            return false;
        };
        unsafe {
            let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let next = if visible {
                (current & !WS_EX_TOOLWINDOW) | WS_EX_APPWINDOW
            } else {
                (current | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW
            };
            // 没变就别动：下面那两步 hide/show 会让窗口闪一下。
            if next == current {
                return true;
            }
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next);
            // 任务栏按钮只在窗口从隐藏变为可见时重建。少了这一下，旧的状态会一直
            // 留着，直到窗口被销毁重建。用 `SW_SHOWNOACTIVATE` 是为了不在切换
            // 置顶这种小动作上把焦点从别人那儿抢走。
            ShowWindow(hwnd, SW_HIDE);
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        true
    }
}

#[cfg(not(windows))]
mod imp {
    use gpui_kit::Window;

    pub fn bring_to_front(_window: &Window) -> bool {
        // macOS 的 `activate` 会 `makeKeyAndOrderFront`，X11 会发 `_NET_ACTIVE_WINDOW`，
        // 两边本来就是对的，交给 gpui。
        false
    }

    pub fn set_taskbar_visible(_window: &Window, _visible: bool) -> bool {
        false
    }
}
