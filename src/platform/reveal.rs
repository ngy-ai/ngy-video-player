//! 在系统文件管理器里「定位」一个文件。
//!
//! 三个平台各有各的说法：Windows 靠 `explorer /select,`，macOS 靠 `open -R`，
//! Linux 没有统一标准——先试 FileManager1 的 D-Bus 接口（它支持选中），
//! 不行就退回 `xdg-open` 打开所在目录。定位失败不算致命错误，返回一句人话即可。

use std::path::Path;

/// 打开文件所在目录并选中它。错误信息可直接给用户看。
pub fn reveal(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("文件已不存在：{}", path.display()));
    }
    imp(path).map_err(|error| format!("无法打开文件管理器：{error}"))
}

#[cfg(windows)]
fn imp(path: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;

    // `explorer` 不按普通程序的规则拆参数：`/select,` 和路径必须拼成**一段**，
    // 且路径自带引号。交给 Command 自动加引号会变成 `"/select,C:\a b\c.mp4"`，
    // explorer 认不出开关，于是只打开「文档」——这个坑踩过不止一次。
    std::process::Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{}\"", path.display()))
        .spawn()?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn imp(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn()?;
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn imp(path: &Path) -> std::io::Result<()> {
    if show_items(path) {
        return Ok(());
    }
    // 没有 FileManager1（或没有文件管理器实现它）时至少把目录打开。
    let directory = path.parent().unwrap_or(path);
    std::process::Command::new("xdg-open")
        .arg(directory)
        .spawn()?;
    Ok(())
}

/// 通过 `org.freedesktop.FileManager1.ShowItems` 打开目录并选中文件。
#[cfg(all(unix, not(target_os = "macos")))]
fn show_items(path: &Path) -> bool {
    let uri = file_uri(path);
    std::process::Command::new("dbus-send")
        .args([
            "--session",
            "--type=method_call",
            "--dest=org.freedesktop.FileManager1",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
            &format!("array:string:{uri}"),
            "string:",
        ])
        .status()
        .is_ok_and(|status| status.success())
}

/// `file:///path/with%20space`。只保留 URL 里不需要转义的字符。
#[cfg(all(unix, not(target_os = "macos")))]
fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        match byte {
            b'/' | b'-' | b'_' | b'.' | b'~' | b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => {
                uri.push(byte as char);
            }
            other => uri.push_str(&format!("%{other:02X}")),
        }
    }
    uri
}
