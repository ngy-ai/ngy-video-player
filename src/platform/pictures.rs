//! Windows 的「图片」库在哪。
//!
//! 不能自己拼 `%USERPROFILE%\Pictures`：那只是默认位置，用户（和装机软件）可以把它
//! 重定向到别的盘——这台机器上的「图片」就被迁到了 `D:\360MoveData\Users\admin\Pictures`。
//! 资源管理器、PowerShell 的 `[Environment]::GetFolderPath('MyPictures')` 都按重定向后的
//! 位置走，截图要是还往 `C:\Users\...\Pictures` 扔，用户在自己的「图片」里根本找不到。
//!
//! 所以这里问同一个 known folder（`FOLDERID_Pictures`）。没有为此引入 `dirs` 之类的依赖：
//! 只要两个 Win32 函数，比拖一棵依赖树下来便宜。

use std::ffi::c_void;
use std::path::PathBuf;

#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

/// `FOLDERID_Pictures` = {33E28130-4E1E-4676-835A-98395C3BC3BB}
const FOLDERID_PICTURES: Guid = Guid {
    data1: 0x33E2_8130,
    data2: 0x4E1E,
    data3: 0x4676,
    data4: [0x83, 0x5A, 0x98, 0x39, 0x5C, 0x3B, 0xC3, 0xBB],
};

#[link(name = "shell32")]
unsafe extern "system" {
    fn SHGetKnownFolderPath(
        folder: *const Guid,
        flags: u32,
        token: *mut c_void,
        path: *mut *mut u16,
    ) -> i32;
}

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoTaskMemFree(ptr: *mut c_void);
}

/// 系统「图片」库的真实位置。重定向过也照样拿到重定向后的那个。
pub fn pictures_dir() -> Option<PathBuf> {
    let mut raw: *mut u16 = std::ptr::null_mut();
    // SAFETY: 三个入参指针都指向本函数的栈上变量 / 空（用默认 token）；
    // 成功时 shell 分配一块以 NUL 结尾的 UTF-16，由下面的 `CoTaskMemFree` 归还。
    let hr = unsafe { SHGetKnownFolderPath(&FOLDERID_PICTURES, 0, std::ptr::null_mut(), &mut raw) };
    if hr < 0 || raw.is_null() {
        return None;
    }

    let mut len = 0usize;
    // SAFETY: 上一步保证 `raw` 指向一块 NUL 结尾的字符串，循环不会越界。
    while unsafe { *raw.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: 同上，`len` 是那串 UTF-16 的长度（不含结尾的 NUL）。
    let wide = unsafe { std::slice::from_raw_parts(raw, len) };
    let path = PathBuf::from(String::from_utf16_lossy(wide));
    // SAFETY: 这块内存是 shell 用 `CoTaskMemAlloc` 分配的，必须用配对的方式释放。
    unsafe { CoTaskMemFree(raw.cast()) };
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 拿到的图片目录是绝对路径() {
        let dir = pictures_dir().expect("Windows 上应当总能拿到图片目录");
        assert!(dir.is_absolute(), "图片目录应当绝对路径：{}", dir.display());
        assert!(!dir.as_os_str().is_empty());
    }
}
