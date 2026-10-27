//! 本机显示适配器（显卡）列表。
//!
//! 「播放显卡设置」要把机器上的显卡列出来，三个平台的接口互不相通：Windows 用
//! `EnumDisplayDevicesW` 一次就够，Linux 的 `/sys/class/drm` 只有 PCI id、拿不到
//! 可读名字，macOS 更没有便宜的公开接口（`system_profiler` 要起进程、几百毫秒）。
//! 所以这里只做「能列就列」：列不出来的平台返回空表，界面显示「系统默认显卡」，
//! 对播放没有任何影响。取舍理由与文件关联、置顶一致：按 `cfg` 分派、不引第三方 crate。

/// 本机显卡名，按系统给出的顺序去重。空表表示这一项在当前平台列不出来。
pub fn adapters() -> Vec<String> {
    imp::adapters()
}

#[cfg(windows)]
mod imp {
    use std::ptr;

    /// `DISPLAY_DEVICEW`（见 wingdi.h）：定长宽字符缓冲区，不是指针。
    #[repr(C)]
    struct DisplayDeviceW {
        cb: u32,
        device_name: [u16; 32],
        device_string: [u16; 128],
        state_flags: u32,
        device_id: [u16; 128],
        device_key: [u16; 128],
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        /// `device` 传空表示「枚举显示适配器」，非空表示枚举该适配器的显示器。
        fn EnumDisplayDevicesW(
            device: *const u16,
            index: u32,
            info: *mut DisplayDeviceW,
            flags: u32,
        ) -> i32;
    }

    /// 定长宽字符缓冲 → `String`，遇到 NUL 就停。
    fn wide_to_string(buffer: &[u16]) -> String {
        let len = buffer
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(buffer.len());
        String::from_utf16_lossy(&buffer[..len])
    }

    pub fn adapters() -> Vec<String> {
        let mut found: Vec<String> = Vec::new();
        // 枚举是「返回 0 即结束」，没有总数可查；给个足够大的上界即可，
        // 正常机器上一两块，工作站也就四块。
        for index in 0..16 {
            let mut info = DisplayDeviceW {
                cb: std::mem::size_of::<DisplayDeviceW>() as u32,
                device_name: [0; 32],
                device_string: [0; 128],
                state_flags: 0,
                device_id: [0; 128],
                device_key: [0; 128],
            };
            let ok = unsafe { EnumDisplayDevicesW(ptr::null(), index, &mut info, 0) };
            if ok == 0 {
                break;
            }
            let name = wide_to_string(&info.device_string);
            // 「Microsoft Basic Render Driver」是没装显卡驱动时的软件兜底，
            // 列出来只会让人以为自己有一块显卡。
            if name.is_empty() || name.contains("Microsoft Basic Render") {
                continue;
            }
            // 一张卡接两个显示器时会重复出现，名字相同只留一条。
            if !found.contains(&name) {
                found.push(name);
            }
        }
        found
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn adapters() -> Vec<String> {
        Vec::new()
    }
}
