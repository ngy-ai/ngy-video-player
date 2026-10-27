//! macOS 上的文件关联。
//!
//! 两道手续缺一不可：bundle 的 `Info.plist` 声明 `CFBundleDocumentTypes`（告诉系统
//! 我们能开什么），再由 LaunchServices 把每种 UTI 的默认处理程序指到本 bundle。
//!
//! 所以这一版必须先有 `.app`：裸二进制在 LaunchServices 眼里没有身份，也没有
//! `CFBundleIdentifier` 可供指认。见 `scripts/bundle-macos.sh`。
//!
//! FFI 全部手写（`docs/adr/0001`）：用到的只有九个符号，比引入 `objc2` 全套运行时
//! 划算得多。

use std::ffi::{CString, c_void};
use std::os::raw::c_char;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use super::{Status, plist};

pub const SUPPORTED: bool = true;

const BUNDLE_ID: &str = "com.ngy.video-player";
const APP_NAME: &str = "ngy-video-player";
const UTF8: u32 = 0x0800_0100;
/// `kLSRolesAll`
const ROLES_ALL: u32 = 0xffff_ffff;

type CFTypeRef = *const c_void;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFStringCreateWithCString(
        allocator: CFTypeRef,
        text: *const c_char,
        encoding: u32,
    ) -> CFTypeRef;
    fn CFURLCreateFromFileSystemRepresentation(
        allocator: CFTypeRef,
        buffer: *const u8,
        length: isize,
        is_directory: u8,
    ) -> CFTypeRef;
    fn CFRelease(value: CFTypeRef);
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn LSSetDefaultRoleHandlerForContentType(
        content_type: CFTypeRef,
        roles: u32,
        handler: CFTypeRef,
    ) -> i32;
    fn LSRegisterURL(url: CFTypeRef, update: u8) -> i32;
}

#[link(name = "UniformTypeIdentifiers", kind = "framework")]
unsafe extern "C" {
    fn UTTypeCreatePreferredIdentifierForTag(
        tag_class: CFTypeRef,
        tag: CFTypeRef,
        conforming_to: CFTypeRef,
    ) -> CFTypeRef;
    static kUTTagClassFilenameExtension: CFTypeRef;
}

/// 持有一个 +1 的 CoreFoundation 对象，离开作用域即释放。
struct Cf(CFTypeRef);

impl Drop for Cf {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

impl Cf {
    fn string(text: &str) -> Result<Self> {
        let text = CString::new(text).context("字符串里混进了 NUL")?;
        let value = unsafe { CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), UTF8) };
        if value.is_null() {
            bail!("创建 CFString 失败");
        }
        Ok(Self(value))
    }

    fn raw(&self) -> CFTypeRef {
        self.0
    }
}

/// 扩展名 → UTI。`UTTypeCreatePreferredIdentifierForTag` 返回 +1 的引用，交给 `Cf` 管。
fn content_type(extension: &str) -> Result<Cf> {
    let tag = Cf::string(extension)?;
    let content_type = unsafe {
        UTTypeCreatePreferredIdentifierForTag(
            kUTTagClassFilenameExtension,
            tag.raw(),
            std::ptr::null(),
        )
    };
    if content_type.is_null() {
        bail!("扩展名 .{extension} 在系统里没有对应的类型");
    }
    Ok(Cf(content_type))
}

/// 当前可执行文件所在的 `.app`。
fn bundle_root() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("取不到当前可执行文件的路径")?;
    // .../Foo.app/Contents/MacOS/Foo
    let macos = exe.parent().context("可执行文件没有上级目录")?;
    let contents = macos.parent().context("可执行文件不在 Contents/MacOS 里")?;
    let bundle = contents.parent().context("可执行文件不在 .app 包内")?;
    if bundle.extension().and_then(|extension| extension.to_str()) != Some("app") {
        bail!(
            "当前不是从 .app 里启动的，无法登记文件关联。\
             请先运行 scripts/bundle-macos.sh 打包，再从包内启动。"
        );
    }
    Ok(bundle.to_path_buf())
}

fn info_plist_path(bundle: &Path) -> PathBuf {
    bundle.join("Contents").join("Info.plist")
}

fn write_info_plist(bundle: &Path, extensions: &[&str]) -> Result<()> {
    let contents = bundle.join("Contents");
    std::fs::create_dir_all(&contents)
        .with_context(|| format!("创建 {} 失败", contents.display()))?;
    // CFBundleExecutable 必须和 Contents/MacOS 下的文件名一致；打包脚本按包名放二进制，
    // 所以这里也按包名算，而不是用 CARGO_PKG_NAME。
    let executable = bundle
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(APP_NAME);
    let text = plist::info_plist(
        BUNDLE_ID,
        executable,
        APP_NAME,
        env!("CARGO_PKG_VERSION"),
        extensions,
    );
    let path = info_plist_path(bundle);
    std::fs::write(&path, text).with_context(|| format!("写 {} 失败", path.display()))
}

fn register_bundle(bundle: &Path) -> Result<()> {
    let path = bundle.to_string_lossy();
    let bytes = path.as_bytes();
    let url = unsafe {
        CFURLCreateFromFileSystemRepresentation(
            std::ptr::null(),
            bytes.as_ptr(),
            bytes.len() as isize,
            1,
        )
    };
    if url.is_null() {
        bail!("创建 bundle 的 CFURL 失败");
    }
    let url = Cf(url);
    let status = unsafe { LSRegisterURL(url.raw(), 1) };
    if status != 0 {
        bail!("把 .app 登记到 LaunchServices 失败（错误码 {status}）");
    }
    Ok(())
}

pub fn register() -> Result<Status> {
    let bundle = bundle_root()?;
    let extensions = crate::formats::associable_extensions();

    write_info_plist(&bundle, &extensions)?;
    register_bundle(&bundle)?;

    let handler = Cf::string(BUNDLE_ID)?;
    let mut applied = 0usize;
    let mut first_failure: Option<(String, i32)> = None;

    for extension in &extensions {
        let Ok(content_type) = content_type(extension) else {
            continue;
        };
        let status = unsafe {
            LSSetDefaultRoleHandlerForContentType(content_type.raw(), ROLES_ALL, handler.raw())
        };
        if status == 0 {
            applied += 1;
        } else if first_failure.is_none() {
            first_failure = Some(((*extension).to_string(), status));
        }
    }

    if applied == 0 {
        let detail = first_failure
            .map(|(extension, status)| format!("第一个失败在 .{extension}（错误码 {status}）"))
            .unwrap_or_else(|| "没有可用的扩展名".to_string());
        bail!("LaunchServices 拒绝了默认处理程序的设置：{detail}");
    }

    Ok(Status {
        registered: applied,
        target: Some(bundle),
    })
}

pub fn status() -> Result<Status> {
    let Ok(bundle) = bundle_root() else {
        return Ok(Status::default());
    };
    let Ok(text) = std::fs::read_to_string(info_plist_path(&bundle)) else {
        return Ok(Status::default());
    };

    // 只报「我们声明了什么」，不去查系统里当前默认是谁：用户把默认改成别的播放器是
    // 他的自由，这里不监工——和 Windows 上不读 `UserChoice` 是同一条原则。
    Ok(Status {
        registered: plist::declared_extensions(&text).len(),
        target: Some(bundle),
    })
}

pub fn remove() -> Result<()> {
    let bundle = bundle_root()?;
    // macOS 没有「把默认还回去」或者「清空默认」的接口，我们只能撤掉声明，
    // 让系统不再把我们当候选。已经选定的默认要由用户自己去「显示简介」里改。
    write_info_plist(&bundle, &[])?;
    register_bundle(&bundle)?;
    Ok(())
}

/// 与 Linux 一致：选谁当默认是 `LSSetDefaultRoleHandlerForContentType` 一句话的事，
/// 不需要把用户送去某个设置页。
pub fn open_system_settings() -> Result<()> {
    Ok(())
}

pub fn guidance() -> &'static str {
    "已经直接成为这些格式的默认播放器。"
}
