//! 用系统的默认程序打开一个文件（截图卡片上的「打开」）。
//!
//! 和「定位」分开：定位是「告诉用户文件在哪」，打开是「把文件内容亮出来」。
//! 三个平台各一句话，不值得为此引入 shell 抽象层。

use std::path::Path;

/// 用默认程序打开 `path`。错误信息可直接给用户看。
pub fn open(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("文件已不存在：{}", path.display()));
    }
    imp(path).map_err(|error| format!("无法打开文件：{error}"))
}

#[cfg(windows)]
fn imp(path: &Path) -> std::io::Result<()> {
    // 不用 `cmd /C start`：那会在 GUI 程序里闪一个控制台窗口。explorer 本来
    // 就在跑，把路径交给它等同于走一遍 ShellExecute，默认程序会被拉起来。
    std::process::Command::new("explorer.exe")
        .arg(path)
        .spawn()?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn imp(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("open").arg(path).spawn()?;
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn imp(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("xdg-open").arg(path).spawn()?;
    Ok(())
}
