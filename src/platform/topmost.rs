//! 窗口置顶。
//!
//! GPUI 没有暴露置顶开关，只能在拿到原生窗口句柄之后自己调平台 API：
//! Win32 改窗口的 TOPMOST 标志，macOS 调 `NSWindow` 的 level，X11 给窗口管理器
//! 发一条 `_NET_WM_STATE_ABOVE` 的 ClientMessage。Wayland 没给普通窗口留这个口子
//! （只有 layer-shell 能压过别的窗口），所以那边一律返回 false，菜单里会把这一组
//! 选项置灰。

use gpui_kit::Window;

/// 当前窗口系统是否支持置顶。只判断窗口句柄的类型，不真的改窗口。
pub fn supported(window: &Window) -> bool {
    imp::supported(window)
}

/// 打开/关闭置顶。返回 `false` 表示这次没能生效——调用方记一条日志即可，不必重试：
/// 不支持是窗口系统层面的结论，不是一次偶发失败。
pub fn set(window: &Window, on_top: bool) -> bool {
    imp::set(window, on_top)
}

/// 把窗口抬到「普通窗口层」的最前：压过所有没置顶的应用，只让置顶窗口压着它。
///
/// 取消置顶之后光靠系统默认的点击激活不够：窗口被别的应用盖住时，激活并不总会
/// 把它抬到那一层的最前。这里显式改一次 Z 序，让「点一下就到最前」成为确定的。
pub fn raise(window: &Window) -> bool {
    imp::raise(window)
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    use gpui_kit::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    /// `SetWindowPos` 的三个特殊「插入位置」：置顶、取消置顶、普通层最前。
    const HWND_TOPMOST: isize = -1;
    const HWND_NOTOPMOST: isize = -2;
    const HWND_TOP: isize = 0;
    const SWP_NOSIZE: u32 = 0x0001;
    const SWP_NOMOVE: u32 = 0x0002;
    const SWP_NOACTIVATE: u32 = 0x0010;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetWindowPos(
            hwnd: *mut c_void,
            insert_after: *mut c_void,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            flags: u32,
        ) -> i32;
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

    pub fn supported(window: &Window) -> bool {
        hwnd(window).is_some()
    }

    pub fn raise(window: &Window) -> bool {
        let Some(hwnd) = hwnd(window) else {
            return false;
        };
        // `HWND_TOP` 是「本条 Z 序带的最前」，不是绝对最前：置顶窗口仍然压得住它，
        // 这正是取消置顶该有的语义。不抢焦点，点击的激活由系统自己处理。
        unsafe {
            SetWindowPos(
                hwnd,
                HWND_TOP as *mut c_void,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            ) != 0
        }
    }

    pub fn set(window: &Window, on_top: bool) -> bool {
        let Some(hwnd) = hwnd(window) else {
            return false;
        };
        // 只改 Z 序：不移动、不缩放、不抢焦点。否则用户正打字时会突然被吞掉一次输入。
        let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
        unsafe {
            let placed = SetWindowPos(
                hwnd,
                if on_top { HWND_TOPMOST } else { HWND_NOTOPMOST } as *mut c_void,
                0,
                0,
                0,
                0,
                flags,
            ) != 0;
            if !on_top {
                // `HWND_NOTOPMOST` 只保证「不被置顶窗口压住」，落点由系统挑。
                // 取消置顶是用户主动做的一次可见操作，窗口不该在同时沉到普通层
                // 的底部去，所以再抬一次到普通层最前。
                SetWindowPos(hwnd, HWND_TOP as *mut c_void, 0, 0, 0, 0, flags);
            }
            placed
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{c_char, c_void};

    use gpui_kit::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    /// `NSWindowLevel`：普通层级与「浮在普通窗口之上」的层级。
    /// 不用更高的档位——那是给面板/弹窗的，会压过用户自己的对话框。
    const NS_NORMAL_WINDOW_LEVEL: isize = 0;
    const NS_FLOATING_WINDOW_LEVEL: isize = 3;

    #[link(name = "objc")]
    unsafe extern "C" {
        fn sel_registerName(name: *const c_char) -> *const c_void;

        // objc_msgSend 在 arm64 上不能当变参函数调（浮点/整数寄存器规则不同），
        // 所以按每种签名各声明一次，用 link_name 指到同一个符号上。
        #[link_name = "objc_msgSend"]
        fn send_window(view: *mut c_void, selector: *const c_void) -> *mut c_void;
        #[link_name = "objc_msgSend"]
        fn send_set_level(window: *mut c_void, selector: *const c_void, level: isize);
    }

    /// 从视图拿它所在的 NSWindow。句柄里给的是 NSView，置顶要设在窗口上。
    fn ns_window(window: &Window) -> Option<*mut c_void> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return None;
        };
        let view = handle.ns_view.as_ptr();
        unsafe {
            let selector = sel_registerName(c"window".as_ptr());
            let window = send_window(view, selector);
            (!window.is_null()).then_some(window)
        }
    }

    pub fn supported(window: &Window) -> bool {
        ns_window(window).is_some()
    }

    pub fn raise(_window: &Window) -> bool {
        // macOS 点一下就由窗口服务器抬到同层最前，不需要额外做什么。
        false
    }

    pub fn set(window: &Window, on_top: bool) -> bool {
        let Some(window) = ns_window(window) else {
            return false;
        };
        let level = if on_top {
            NS_FLOATING_WINDOW_LEVEL
        } else {
            NS_NORMAL_WINDOW_LEVEL
        };
        unsafe {
            let selector = sel_registerName(c"setLevel:".as_ptr());
            send_set_level(window, selector, level);
        }
        true
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use std::ffi::{c_char, c_int, c_long, c_ulong, c_void};
    use std::ptr;

    use gpui_kit::Window;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    /// `XClientMessageEvent`（见 Xlib.h）。
    #[repr(C)]
    struct ClientMessageEvent {
        kind: c_int,
        serial: c_ulong,
        send_event: c_int,
        display: *mut c_void,
        window: c_ulong,
        message_type: c_ulong,
        format: c_int,
        data: [c_long; 5],
    }

    const CLIENT_MESSAGE: c_int = 33;
    /// `SubstructureNotifyMask | SubstructureRedirectMask`：窗口管理器监听根窗口用的掩码。
    const SUBSTRUCTURE_MASKS: c_long = (1 << 19) | (1 << 20);
    const NET_WM_STATE_REMOVE: c_long = 0;
    const NET_WM_STATE_ADD: c_long = 1;
    /// EWMH 的 source indication：1 表示普通应用（0 是旧的、已废弃的写法）。
    const SOURCE_APPLICATION: c_long = 1;
    /// `XEvent` 是联合体，libX11 按地址读写；64 位下它占 24 个 `long`，32 位下也是 24 个
    /// （尺寸由 libX11 决定，这里按最大值留缓冲区，两边都够用）。
    const EVENT_WORDS: usize = 24;

    #[link(name = "X11")]
    unsafe extern "C" {
        fn XOpenDisplay(name: *const c_char) -> *mut c_void;
        fn XDefaultScreen(display: *mut c_void) -> c_int;
        fn XRootWindow(display: *mut c_void, screen: c_int) -> c_ulong;
        fn XInternAtom(display: *mut c_void, name: *const c_char, only_if_exists: c_int)
        -> c_ulong;
        fn XSendEvent(
            display: *mut c_void,
            window: c_ulong,
            propagate: c_int,
            event_mask: c_long,
            event: *mut c_void,
        ) -> c_int;
        fn XFlush(display: *mut c_void) -> c_int;
        fn XCloseDisplay(display: *mut c_void) -> c_int;
    }

    /// X11 的窗口 id；Wayland 下 GPUI 给的是 `wl_surface`，没有置顶这个概念。
    fn x11_window(window: &Window) -> Option<c_ulong> {
        let handle = HasWindowHandle::window_handle(window).ok()?;
        match handle.as_raw() {
            RawWindowHandle::Xcb(handle) => Some(c_ulong::from(handle.window.get())),
            RawWindowHandle::Xlib(handle) => Some(handle.window),
            _ => None,
        }
    }

    pub fn supported(window: &Window) -> bool {
        x11_window(window).is_some()
    }

    pub fn raise(_window: &Window) -> bool {
        // X11 下点击激活由窗口管理器统一处理，抬升不需要应用插手。
        false
    }

    pub fn set(window: &Window, on_top: bool) -> bool {
        let Some(window_id) = x11_window(window) else {
            return false;
        };

        // 用独立的 Xlib 连接发消息，避免和 GPUI 自己的 XCB 连接抢事件队列。
        // 只在用户切换置顶时调用一次，开一次连接的开销可以接受。
        unsafe {
            let display = XOpenDisplay(ptr::null());
            if display.is_null() {
                return false;
            }
            let root = XRootWindow(display, XDefaultScreen(display));
            let state = XInternAtom(display, c"_NET_WM_STATE".as_ptr(), 0);
            let above = XInternAtom(display, c"_NET_WM_STATE_ABOVE".as_ptr(), 0);
            if state == 0 || above == 0 {
                XCloseDisplay(display);
                return false;
            }

            let mut event = [0 as c_long; EVENT_WORDS];
            let client = event.as_mut_ptr() as *mut ClientMessageEvent;
            client.write(ClientMessageEvent {
                kind: CLIENT_MESSAGE,
                serial: 0,
                // Xlib 的 Bool：非 0 表示这条事件由 SendEvent 合成。
                send_event: 1,
                display,
                window: window_id,
                message_type: state,
                format: 32,
                data: [
                    if on_top {
                        NET_WM_STATE_ADD
                    } else {
                        NET_WM_STATE_REMOVE
                    },
                    above as c_long,
                    0,
                    SOURCE_APPLICATION,
                    0,
                ],
            });

            let sent = XSendEvent(
                display,
                root,
                0,
                SUBSTRUCTURE_MASKS,
                event.as_mut_ptr() as *mut c_void,
            );
            XFlush(display);
            XCloseDisplay(display);
            sent != 0
        }
    }
}
