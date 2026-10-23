//! 把 `assets/icon.ico` 嵌进可执行文件。
//!
//! Windows 的窗口与任务栏图标取自 exe 的资源段，gpui 的 `WindowOptions::icon`
//! 只对 X11 生效，所以只能在构建这一层嵌。
//!
//! 嵌入失败只发 warning、不中断构建：图标是装饰，不该因为机器上找不到 `rc.exe`
//! 就让整个项目编译不过。

#[cfg(windows)]
use std::path::{Path, PathBuf};

fn main() {
    // 监视整个目录而不是单个文件：图标是脚本生成的，首次构建可能还不存在。
    // 只监视 assets/icon.ico 的话，cargo 会把「缺失」记进指纹，之后生成图标
    // 也不会重跑本脚本。
    println!("cargo:rerun-if-changed=assets");

    #[cfg(windows)]
    embed_icon();
}

#[cfg(windows)]
fn embed_icon() {
    let icon: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("icon.ico");
    if !icon.exists() {
        println!("cargo:warning=assets/icon.ico 不存在，跳过图标嵌入");
        return;
    }

    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.1.0".to_string());

    let mut resource = winresource::WindowsResource::new();
    resource.set_icon(&icon.to_string_lossy());
    resource.set("FileDescription", "跨平台视频播放器");
    resource.set("ProductName", "影匣");
    resource.set("FileVersion", &format!("{version}.0"));
    resource.set("ProductVersion", &format!("{version}.0"));
    resource.set("LegalCopyright", "Apache-2.0");

    if let Err(error) = resource.compile() {
        println!("cargo:warning=嵌入图标失败（不影响构建）: {error}");
    }
}
