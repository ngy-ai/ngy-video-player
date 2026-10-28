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

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    use gpui_kit::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    const SW_RESTORE: i32 = 9;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn ShowWindow(hwnd: *mut c_void, cmd: i32) -> i32;
        fn BringWindowToTop(hwnd: *mut c_void) -> i32;
        fn SetForegroundWindow(hwnd: *mut c_void) -> i32;
        fn SetActiveWindow(hwnd: *mut c_void) -> *mut c_void;
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
}

#[cfg(not(windows))]
mod imp {
    use gpui_kit::Window;

    pub fn bring_to_front(_window: &Window) -> bool {
        // macOS 的 `activate` 会 `makeKeyAndOrderFront`，X11 会发 `_NET_ACTIVE_WINDOW`，
        // 两边本来就是对的，交给 gpui。
        false
    }
}
