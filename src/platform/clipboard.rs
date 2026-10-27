//! 把截图放进系统剪贴板。
//!
//! 三个平台的剪贴板能力并不对等：Windows 与 macOS 会真的写一张图片进去，
//! Linux 的 X11 和 Wayland 后端都只写文本，图片条目会被**静默丢掉**——那比
//! 复制失败更糟，用户会以为粘不出来是自己操作错了。所以 Linux 上退一步复制
//! 文件路径，按钮文案也跟着改，别让人误会。

use gpui_kit::ClipboardItem;
use std::path::Path;

/// 复制按钮上的文案。Linux 复制的是路径，必须说清楚。
#[cfg(any(windows, target_os = "macos"))]
pub const COPY_LABEL: &str = "复制";
#[cfg(all(unix, not(target_os = "macos")))]
pub const COPY_LABEL: &str = "复制路径";

/// 复制成功后的提示，与上面的文案成对。
#[cfg(any(windows, target_os = "macos"))]
pub const COPIED_MESSAGE: &str = "截图已复制到剪贴板";
#[cfg(all(unix, not(target_os = "macos")))]
pub const COPIED_MESSAGE: &str = "截图路径已复制到剪贴板";

/// 准备一份可以直接写进剪贴板的数据。
#[cfg(any(windows, target_os = "macos"))]
pub fn item(path: &Path) -> anyhow::Result<ClipboardItem> {
    let bytes = std::fs::read(path)
        .map_err(|error| anyhow::anyhow!("读取截图失败（{}）：{error}", path.display()))?;
    Ok(ClipboardItem::new_image(&gpui_kit::Image::from_bytes(
        gpui_kit::ImageFormat::Png,
        bytes,
    )))
}

/// 准备一份可以直接写进剪贴板的数据。
#[cfg(all(unix, not(target_os = "macos")))]
pub fn item(path: &Path) -> anyhow::Result<ClipboardItem> {
    if !path.exists() {
        return Err(anyhow::anyhow!("截图已不存在：{}", path.display()));
    }
    Ok(ClipboardItem::new_string(path.display().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 不存在的截图不产生剪贴板内容() {
        let missing = Path::new("这个文件一定不存在-截图.png");
        assert!(item(missing).is_err());
    }
}
