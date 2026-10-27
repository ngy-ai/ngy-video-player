//! Windows 上的文件关联。
//!
//! 合法性边界见 `docs/adr/0002`：只写 `HKCU` 下的 ProgID / Capabilities /
//! RegisteredApplications，让本程序出现在「默认应用」列表里，然后请用户去系统设置
//! 亲手选定。`FileExts\<ext>\UserChoice` 带哈希校验，写它会被判成篡改并被重置，
//! 本文件绝不碰。
//!
//! 这里手写 Win32 FFI 而不是引入 `windows-sys`：真正用到的只有七个函数，写成一段
//! 一眼能读完的声明，比多扛一份几千个符号的绑定更划算（`docs/adr/0001`）。

use std::ffi::c_void;
use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};

use super::Status;

pub const SUPPORTED: bool = true;

/// 所有扩展名共用一个 ProgID。
///
/// 它们是同一种东西——「能交给本程序播放的文件」。按扩展名各建一个 ProgID 只会在
/// 注册表里铺出几十份内容完全相同的键，还得逐个删干净。
const PROG_ID: &str = "NgyVideoPlayer.video";
const APP_NAME: &str = "ngy-video-player";
const APP_DESCRIPTION: &str = "轻量视频播放器（GPUI + libmpv）";
/// `HKCU\Software\RegisteredApplications` 里登记的能力键路径。
const CAPABILITIES: &str = r"Software\ngy-video-player\Capabilities";
const REGISTERED_APPLICATIONS: &str = r"Software\RegisteredApplications";

// 名字照抄 Win32 SDK：读代码时能直接对上官方文档，比自创别名省事。
#[allow(non_camel_case_types, clippy::upper_case_acronyms)]
type HKEY = *mut c_void;
#[allow(non_camel_case_types, clippy::upper_case_acronyms)]
type LSTATUS = i32;

const HKEY_CURRENT_USER: HKEY = 0x8000_0001usize as HKEY;
const KEY_READ: u32 = 0x0002_0019;
const KEY_WRITE: u32 = 0x0002_0006;
const REG_SZ: u32 = 1;
const REG_NONE: u32 = 0;
const ERROR_SUCCESS: LSTATUS = 0;
const ERROR_FILE_NOT_FOUND: LSTATUS = 2;
const ERROR_PATH_NOT_FOUND: LSTATUS = 3;

#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegCreateKeyExW(
        hkey: HKEY,
        sub_key: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        sam: u32,
        security: *const c_void,
        result: *mut HKEY,
        disposition: *mut u32,
    ) -> LSTATUS;

    fn RegOpenKeyExW(
        hkey: HKEY,
        sub_key: *const u16,
        options: u32,
        sam: u32,
        result: *mut HKEY,
    ) -> LSTATUS;

    fn RegSetValueExW(
        hkey: HKEY,
        name: *const u16,
        reserved: u32,
        kind: u32,
        data: *const u8,
        len: u32,
    ) -> LSTATUS;

    fn RegQueryValueExW(
        hkey: HKEY,
        name: *const u16,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        len: *mut u32,
    ) -> LSTATUS;

    fn RegDeleteTreeW(hkey: HKEY, sub_key: *const u16) -> LSTATUS;

    fn RegDeleteKeyValueW(hkey: HKEY, sub_key: *const u16, name: *const u16) -> LSTATUS;

    fn RegCloseKey(hkey: HKEY) -> LSTATUS;
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn ShellExecuteW(
        window: *mut c_void,
        operation: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show: i32,
    ) -> *mut c_void;
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn name_ptr(name: &Option<Vec<u16>>) -> *const u16 {
    name.as_ref()
        .map_or(std::ptr::null(), |value| value.as_ptr())
}

/// 打开的键在离开作用域时自动关闭，省得每条返回路径都记得关。
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}

impl Key {
    fn create(path: &str) -> Result<Self> {
        let sub_key = wide(path);
        let mut handle: HKEY = std::ptr::null_mut();
        let mut disposition = 0u32;
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                sub_key.as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_WRITE,
                std::ptr::null(),
                &mut handle,
                &mut disposition,
            )
        };
        if status != ERROR_SUCCESS {
            bail!("写入注册表 HKCU\\{path} 失败（错误码 {status}）");
        }
        Ok(Self(handle))
    }

    fn open(path: &str) -> Option<Self> {
        let sub_key = wide(path);
        let mut handle: HKEY = std::ptr::null_mut();
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                sub_key.as_ptr(),
                0,
                KEY_READ,
                &mut handle,
            )
        };
        (status == ERROR_SUCCESS).then_some(Self(handle))
    }

    fn set(&self, name: Option<&str>, value: &str) -> Result<()> {
        let name = name.map(wide);
        let data = wide(value);
        let status = unsafe {
            RegSetValueExW(
                self.0,
                name_ptr(&name),
                0,
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        };
        if status != ERROR_SUCCESS {
            bail!("写入注册表值失败（错误码 {status}）");
        }
        Ok(())
    }

    /// 写一个空值的 `REG_NONE`。`OpenWithProgids` 只认「值存不存在」，不看内容。
    fn add_marker(&self, name: &str) -> Result<()> {
        let name = wide(name);
        let status =
            unsafe { RegSetValueExW(self.0, name.as_ptr(), 0, REG_NONE, std::ptr::null(), 0) };
        if status != ERROR_SUCCESS {
            bail!("写入注册表标记失败（错误码 {status}）");
        }
        Ok(())
    }

    fn has(&self, name: &str) -> bool {
        let name = wide(name);
        let mut len = 0u32;
        let status = unsafe {
            RegQueryValueExW(
                self.0,
                name.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut len,
            )
        };
        status == ERROR_SUCCESS
    }

    /// 读字符串值；`None` 表示键的默认值。
    fn get(&self, name: Option<&str>) -> Option<String> {
        let name = name.map(wide);
        let mut len = 0u32;
        let mut kind = 0u32;
        let status = unsafe {
            RegQueryValueExW(
                self.0,
                name_ptr(&name),
                std::ptr::null_mut(),
                &mut kind,
                std::ptr::null_mut(),
                &mut len,
            )
        };
        if status != ERROR_SUCCESS || len < 2 {
            return None;
        }
        let mut buffer = vec![0u16; len as usize / 2 + 1];
        let status = unsafe {
            RegQueryValueExW(
                self.0,
                name_ptr(&name),
                std::ptr::null_mut(),
                &mut kind,
                buffer.as_mut_ptr().cast(),
                &mut len,
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        while buffer.last() == Some(&0) {
            buffer.pop();
        }
        Some(String::from_utf16_lossy(&buffer))
    }
}

fn prog_id_path() -> String {
    format!(r"Software\Classes\{PROG_ID}")
}

fn open_with_path(extension: &str) -> String {
    format!(r"Software\Classes\.{extension}\OpenWithProgids")
}

fn delete_tree(path: &str) -> Result<()> {
    let wide_path = wide(path);
    let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide_path.as_ptr()) };
    // 本来就不存在（第一次撤销、或者被用户手动清掉了）不算失败：撤销要能反复执行。
    if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND && status != ERROR_PATH_NOT_FOUND {
        bail!("删除注册表 HKCU\\{path} 失败（错误码 {status}）");
    }
    Ok(())
}

fn delete_value(path: &str, name: &str) -> Result<()> {
    let wide_path = wide(path);
    let wide_name = wide(name);
    let status =
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, wide_path.as_ptr(), wide_name.as_ptr()) };
    if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND && status != ERROR_PATH_NOT_FOUND {
        bail!("从注册表 HKCU\\{path} 删除值 {name} 失败（错误码 {status}）");
    }
    Ok(())
}

/// 从 `"C:\path\app.exe" "%1"` 里把 exe 路径抠出来。
fn parse_command_path(command: &str) -> Option<PathBuf> {
    let command = command.trim();
    if let Some(rest) = command.strip_prefix('"') {
        let end = rest.find('"')?;
        Some(PathBuf::from(&rest[..end]))
    } else {
        command
            .split_whitespace()
            .next()
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
    }
}

pub fn register() -> Result<Status> {
    let exe = std::env::current_exe().context("取不到当前可执行文件的路径")?;
    let exe = exe.to_string_lossy().to_string();
    let extensions = crate::formats::associable_extensions();

    // 1. ProgID —— 双击之后到底怎么启动，由这里决定。
    let prog_id = Key::create(&prog_id_path())?;
    prog_id.set(None, &format!("{APP_DESCRIPTION} 文件"))?;
    prog_id.set(Some("FriendlyTypeName"), APP_DESCRIPTION)?;

    let icon = Key::create(&format!(r"{}\DefaultIcon", prog_id_path()))?;
    icon.set(None, &format!("\"{exe}\",0"))?;

    let command = Key::create(&format!(r"{}\shell\open\command", prog_id_path()))?;
    command.set(None, &format!("\"{exe}\" \"%1\""))?;

    // 2. 让每类文件在「打开方式」里出现。
    //    只写 OpenWithProgids 子键，不碰 `.ext` 自身的默认值，所以不会抢走当前默认。
    for extension in &extensions {
        Key::create(&open_with_path(extension))?.add_marker(PROG_ID)?;
    }

    // 3. Capabilities —— 让本程序出现在「默认应用」列表里，用户才选得到。
    let capabilities = Key::create(CAPABILITIES)?;
    capabilities.set(Some("ApplicationName"), APP_NAME)?;
    capabilities.set(Some("ApplicationDescription"), APP_DESCRIPTION)?;
    capabilities.set(Some("ApplicationIcon"), &format!("\"{exe}\",0"))?;

    let associations = Key::create(&format!(r"{CAPABILITIES}\FileAssociations"))?;
    for extension in &extensions {
        associations.set(Some(&format!(".{extension}")), PROG_ID)?;
    }

    // 4. 把能力键挂到当前用户的应用清单上。
    Key::create(REGISTERED_APPLICATIONS)?.set(Some(APP_NAME), CAPABILITIES)?;

    Ok(Status {
        registered: extensions.len(),
        target: Some(PathBuf::from(exe)),
    })
}

pub fn remove() -> Result<()> {
    for extension in crate::formats::associable_extensions() {
        delete_value(&open_with_path(extension), PROG_ID)?;
    }
    delete_tree(&prog_id_path())?;
    delete_tree(r"Software\ngy-video-player")?;
    delete_value(REGISTERED_APPLICATIONS, APP_NAME)?;
    Ok(())
}

pub fn status() -> Result<Status> {
    let extensions = crate::formats::associable_extensions();
    let mut registered = 0usize;
    for extension in &extensions {
        if Key::open(&open_with_path(extension)).is_some_and(|key| key.has(PROG_ID)) {
            registered += 1;
        }
    }

    let target = Key::open(&format!(r"{}\shell\open\command", prog_id_path()))
        .and_then(|key| key.get(None))
        .and_then(|command| parse_command_path(&command));

    Ok(Status { registered, target })
}

/// Windows 只能把用户送到设置页，最后那一下必须由用户亲手点（`docs/adr/0002`）。
pub fn guidance() -> &'static str {
    "请在打开的「默认应用」里选择 ngy-video-player。"
}

pub fn open_system_settings() -> Result<()> {
    // Win11 22H2 之后认 `registeredAppUser`，能直接把设置页滚到本程序那一行；
    // 旧系统会忽略查询串，停在「默认应用」首页，不会失败。
    let targets = [
        format!("ms-settings:defaultapps?registeredAppUser={APP_NAME}"),
        "ms-settings:defaultapps".to_string(),
    ];

    let operation = wide("open");
    for target in targets {
        let file = wide(&target);
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                operation.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
            )
        };
        if result as usize > 32 {
            return Ok(());
        }
    }
    bail!("打不开系统设置页，请手动打开 设置 > 应用 > 默认应用")
}
