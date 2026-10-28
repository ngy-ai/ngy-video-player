//! 构建期准备：图标资源与内嵌的 libmpv。
//!
//! 1. 把 `assets/icon.ico` 嵌进可执行文件。Windows 的窗口与任务栏图标取自 exe 的
//!    资源段，gpui 的 `WindowOptions::icon` 只对 X11 生效，所以只能在构建这一层嵌。
//! 2. release 构建把 `libs/<平台>/` 下的 libmpv 内嵌进可执行文件，做出「一个 exe
//!    就能播」的分发形态（见 `src/player/embed.rs`、`docs/adr/0005`）。
//!
//! 两件事都「尽力而为」：图标嵌不进去、库找不到，都只发 warning 不中断构建。
//! 缺库时程序本来就能启动并说明缺什么，不该因此让整个项目编译不过。

use std::path::{Path, PathBuf};

/// 本脚本生成的模块，由 `src/player/embed.rs` 用 `include!` 拉进来。
const GENERATED: &str = "mpv_embed.rs";

fn main() {
    // 监视整个目录而不是单个文件：图标是脚本生成的，首次构建可能还不存在。
    // 只监视 assets/icon.ico 的话，cargo 会把「缺失」记进指纹，之后生成图标
    // 也不会重跑本脚本。
    println!("cargo:rerun-if-changed=assets");
    // 换掉 libs/ 里的库就要重新内嵌，同理监视整个目录。
    println!("cargo:rerun-if-changed=libs");
    println!("cargo:rerun-if-env-changed=NGY_EMBED_MPV");

    #[cfg(windows)]
    embed_icon();

    embed_libmpv();
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

/// 生成 `mpv_embed.rs`：这次构建要不要把 libmpv 一起塞进二进制。
fn embed_libmpv() {
    let generated =
        PathBuf::from(std::env::var("OUT_DIR").expect("cargo 未提供 OUT_DIR")).join(GENERATED);

    match embed_source() {
        Some(source) => {
            write_generated(&generated, Some(&source));
            println!(
                "cargo:warning=已内嵌 {}（{:.1} MB）：release 产物只有一个 exe，不需要旁边的 dll",
                source.display(),
                size_mb(&source)
            );
        }
        // 没有库也要生成一份空表：src 侧只有一个 `include!` 入口，
        // 不该为了「这次没内嵌」分叉出两套代码。
        None => write_generated(&generated, None),
    }
}

/// 决定内嵌哪一份库。
///
/// 优先级：`NGY_EMBED_MPV`（显式指定路径；设为 `0` 表示不内嵌）→ `libs/<平台>/`。
/// 只有 release 构建会自动内嵌：开发期天天改代码，每次链接都搬运上百 MB 没有意义，
/// 而且 `libs/` 就在项目里，`cargo run` 运行时自己找得到。
fn embed_source() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("NGY_EMBED_MPV") {
        if explicit.is_empty() || explicit == "0" {
            return None;
        }
        let path = PathBuf::from(&explicit);
        if path.is_file() {
            return Some(path);
        }
        println!("cargo:warning=NGY_EMBED_MPV={explicit} 不是文件，本次不内嵌 libmpv");
        return None;
    }

    if std::env::var("PROFILE").as_deref() != Ok("release") {
        return None;
    }

    let libs = Path::new(&std::env::var("CARGO_MANIFEST_DIR").ok()?)
        .join("libs")
        .join(platform_dir());
    let found = lib_names()
        .iter()
        .map(|name| libs.join(name))
        .find(|candidate| candidate.is_file());

    if found.is_none() {
        println!(
            "cargo:warning=release 构建没找到 libmpv（{}），产物不含内置库；\
             先运行 scripts/fetch-libmpv.sh 再构建，才能做到单文件分发",
            libs.display()
        );
    }
    found
}

/// 写 `mpv_embed.rs`。`source` 为 `None` 时写成「没有内嵌」的空表。
fn write_generated(path: &Path, source: Option<&Path>) {
    let content = match source {
        None => "// 由 build.rs 生成，不要手改。\n\
                  pub static NAME: Option<&str> = None;\n\
                  pub static BYTES: Option<&[u8]> = None;\n"
            .to_string(),
        Some(source) => {
            let name = source
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            // 路径里的反斜杠在字符串字面量里要转义；正斜杠三个平台的加载器都认，
            // 换成它就不用纠结转义，中文目录也能原样写进去。
            let literal = format!("{:?}", source.to_string_lossy().replace('\\', "/"));
            format!(
                "// 由 build.rs 生成，不要手改。\n\
                 pub static NAME: Option<&str> = Some({name:?});\n\
                 pub static BYTES: Option<&[u8]> = Some(include_bytes!({literal}));\n"
            )
        }
    };

    if let Err(error) = std::fs::write(path, content) {
        println!("cargo:warning=写入 {} 失败: {error}", path.display());
    }
}

fn size_mb(path: &Path) -> f64 {
    std::fs::metadata(path)
        .map(|meta| meta.len() as f64 / (1024.0 * 1024.0))
        .unwrap_or(0.0)
}

// 下面两个函数与 `src/player/ffi.rs` 里的同名逻辑保持一致：构建脚本是独立编译的
// 程序，没法复用 crate 里的代码，改一处就要同步另一处。

fn platform_dir() -> &'static str {
    if cfg!(windows) {
        if cfg!(target_arch = "aarch64") {
            "windows-arm64"
        } else {
            "windows-x86_64"
        }
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "macos-arm64"
        } else {
            "macos-x86_64"
        }
    } else if cfg!(target_arch = "aarch64") {
        "linux-arm64"
    } else {
        "linux-x86_64"
    }
}

fn lib_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["libmpv-2.dll", "libmpv.dll", "mpv-2.dll"]
    } else if cfg!(target_os = "macos") {
        &["libmpv.2.dylib", "libmpv.dylib"]
    } else {
        &["libmpv.so.2", "libmpv.so.1", "libmpv.so"]
    }
}
