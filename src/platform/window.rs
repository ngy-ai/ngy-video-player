//! 窗口的收放与置前：`bring_to_front` / `minimize` / `hide`。
//!
//! gpui 的 `Window::activate_window()` 在 Windows 上只做 `ShowWindowAsync(SW_RESTORE)`
//! ＋ `SetActiveWindow` ＋ `SetFocus`。前一个是异步的，后两个是**线程内**的概念：
//! 它们能把窗口从最小化状态恢复，却不会把窗口抬到别的进程之上。于是「从小窗还原
//! 主窗口」就会还原成功但停在别的应用后面，看起来像没反应。
//!
//! 这里补上真正管用的那两步：`BringWindowToTop` 改 Z 序、`SetForegroundWindow` 抢前台。
//! 后者受 Windows 的前台锁限制（只有当前前台进程才能调用成功），而我们的调用点恰好
//! 都发生在用户刚点过本程序的窗口之后，此时进程就是前台，能过。
//!
//! 收起窗口的两个同步版本也是为小窗模式准备的：gpui 只有异步的 `minimize_window`，
//! 也没有隐藏单窗口的 API。`minimize` 赶在同一拍里最小化，`hide` 干脆把窗口从屏幕上
//! 撤下来——连任务栏那一条一起消失。

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

/// 这个平台能不能把窗口「撤下来」（藏起来，而不是最小化）。
///
/// 撤下来的窗口连任务栏里那一条一起消失，于是「点任务栏」这个动作根本不存在。
/// 小窗模式正需要这一点：任务栏上只留小窗那一条，用户点它时系统要还原的是小窗
/// 本身，主窗口不会被翻出来。
///
/// 最小化的主窗口做不到。Win32 的任务栏按钮一定会先把窗口还原，再把结果交给
/// 我们的可见性观察者；中间那一帧是系统画出来的，没有「拒绝还原」的入口
/// （拦 `WM_SYSCOMMAND` 也不行：任务栏走的是内部还原，压根不发这条消息），
/// 事后同步最小化只能缩短、不能消除它。
///
/// macOS / X11 上点 Dock 图标或应用图标不会把最小化的窗口翻出来，没有这个问题，
/// 所以那边保持最小化，小窗也仍然是工具窗口（见 ADR 0004）。
pub fn can_hide() -> bool {
    imp::CAN_HIDE
}

/// 把窗口从屏幕上撤下来，任务栏 / Alt+Tab 里不再有它。
///
/// 返回 `false` 表示这个平台没有对应实现，调用方退回 `Window::minimize_window()`。
/// 重新显示不用专门的方法：`bring_to_front` 会按窗口当前状态挑 `SW_SHOW` 或
/// `SW_RESTORE`。
pub fn hide(window: &Window) -> bool {
    imp::hide(window)
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    use gpui_kit::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    const SW_HIDE: i32 = 0;
    const SW_SHOW: i32 = 5;
    const SW_MINIMIZE: i32 = 6;
    const SW_RESTORE: i32 = 9;

    pub const CAN_HIDE: bool = true;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn ShowWindow(hwnd: *mut c_void, cmd: i32) -> i32;
        fn IsIconic(hwnd: *mut c_void) -> i32;
        fn IsWindowVisible(hwnd: *mut c_void) -> i32;
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
            // 顺序有讲究：不可见的窗口不先显示出来，后面几步作用的都是一个看不见
            // 的窗口；先显示、再改 Z 序，最后才是抢前台。
            //
            // 三种状态分开处理，`SW_RESTORE` 并不是万能的：
            //  * 最小化 → `SW_RESTORE`，回到最小化之前的样子；
            //  * 藏着（`hide` 过的）→ `SW_SHOW`。这里**不能**用 `SW_RESTORE`：
            //    对藏起来的最大化窗口它会顺手取消最大化，用户的窗口尺寸就这么丢了。
            //  * 已经显示 → 什么都不做。
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            } else if IsWindowVisible(hwnd) == 0 {
                ShowWindow(hwnd, SW_SHOW);
            }
            BringWindowToTop(hwnd);
            SetForegroundWindow(hwnd);
            SetActiveWindow(hwnd);
        }
        // 抢前台失败通常只是前台锁在拦，不代表平台不支持；此时窗口至少已经显示
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

    pub fn hide(window: &Window) -> bool {
        let Some(hwnd) = hwnd(window) else {
            return false;
        };
        // `SW_HIDE` 清掉 `WS_VISIBLE`，任务栏那一条随之消失；窗口的位置、尺寸、
        // 最大化状态都原样留着，`bring_to_front` 里的 `SW_SHOW` 回来还是它原来的样子。
        // 不激活别的窗口，焦点交给系统自己挑（这时该拿到焦点的正是刚建出来的小窗）。
        unsafe { ShowWindow(hwnd, SW_HIDE) };
        true
    }
}

#[cfg(not(windows))]
mod imp {
    use gpui_kit::Window;

    /// 见 `can_hide` 的文档：这两个平台的激活路径不会把最小化的窗口翻出来，
    /// 也就不需要「撤下来」这一招。真要做的话，macOS 要 `orderOut:` 再补一次
    /// `makeKeyAndOrderFront`，X11 要 unmap 再自己 map 回去——多一套没验证过的
    /// 路径，换不来任何改善。
    pub const CAN_HIDE: bool = false;

    pub fn bring_to_front(_window: &Window) -> bool {
        // macOS 的 `activate` 会 `makeKeyAndOrderFront`，X11 会发 `_NET_ACTIVE_WINDOW`，
        // 两边本来就是对的，交给 gpui。
        false
    }

    pub fn minimize(_window: &Window) -> bool {
        // 两边的 `minimize` 都只是把请求交给窗口服务器，没有「晚一拍」的问题。
        false
    }

    pub fn hide(_window: &Window) -> bool {
        false
    }
}
