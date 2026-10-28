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

/// 立即把窗口收起来。返回 `false` 表示这个平台没有对应实现，调用方退回
/// `Window::minimize_window()`。
///
/// gpui 的 `minimize_window` 走 `ShowWindowAsync`：请求投出去就返回，窗口要等
/// 下一轮消息循环才真的收起。小窗模式下要赶在「主窗口刚被系统还原」的同一拍把
/// 它按回去，晚一拍就意味着主窗口实打实地闪出来一次。
pub fn minimize(window: &Window) -> bool {
    imp::minimize(window)
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    use gpui_kit::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    const SW_RESTORE: i32 = 9;
    const SW_MINIMIZE: i32 = 6;

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

    pub fn minimize(window: &Window) -> bool {
        let Some(hwnd) = hwnd(window) else {
            return false;
        };
        // 同步版：`ShowWindow` 会当场派发 `WM_SIZE`，不等下一轮消息循环。
        unsafe { ShowWindow(hwnd, SW_MINIMIZE) };
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

    pub fn minimize(_window: &Window) -> bool {
        // 两边的 `minimize` 都只是把请求交给窗口服务器，没有「晚一拍」的问题。
        false
    }
}
