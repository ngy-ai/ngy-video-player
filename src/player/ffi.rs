//! libmpv 的运行时绑定。
//!
//! 这里刻意**不链接** libmpv，也不依赖它的头文件参与构建：库在进程启动后由
//! [`MpvApi::load`] 用 `dlopen`/`LoadLibrary` 找出来。理由有三个：
//!
//! 1. 构建机不需要装 libmpv，CI 不需要 C 工具链，交叉编译也不会被卡住；
//! 2. 三大平台的分发形态不一样（Windows 是 `libmpv-2.dll`，macOS 是 dylib，
//!    Linux 走发行版的 `libmpv.so.2`），运行期决定更省事；
//! 3. 库缺失时可以给用户一句能读懂的话，而不是让整个程序在加载阶段就死掉。
//!
//! release 构建还会把库内嵌进 exe（见 [`super::embed`]），所以「旁边没有 dll」不再是
//! 失败条件：磁盘上找不到时解压一份内置的出来加载。
//!
//! 只声明这个播放器真正用到的符号。参数与返回值严格按 mpv 官方 `client.h` /
//! `render.h` 写，结构体布局也照抄 C 定义。

use super::embed;
use anyhow::{Context as _, Result, anyhow, bail};
use libloading::Library;
use std::{
    ffi::{CStr, CString, c_char, c_double, c_int, c_ulong, c_void},
    path::{Path, PathBuf},
    sync::Arc,
};

// ---------------------------------------------------------------------------
// C 侧类型
// ---------------------------------------------------------------------------

/// `mpv_handle` 是不完整类型，我们只持有指针。
#[repr(C)]
pub struct MpvHandle {
    _private: [u8; 0],
}

/// `mpv_render_context` 同上。
#[repr(C)]
pub struct MpvRenderContext {
    _private: [u8; 0],
}

#[repr(C)]
pub struct MpvEvent {
    pub event_id: c_int,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct MpvEventProperty {
    pub name: *const c_char,
    pub format: c_int,
    pub data: *mut c_void,
}

#[repr(C)]
pub struct MpvEventEndFile {
    pub reason: c_int,
    pub error: c_int,
    pub playlist_entry_id: i64,
    pub playlist_insert_id: i64,
    pub playlist_insert_num_entries: c_int,
}

#[repr(C)]
pub struct MpvEventLogMessage {
    pub prefix: *const c_char,
    pub level: *const c_char,
    pub text: *const c_char,
    pub log_level: c_int,
}

#[repr(C)]
pub struct MpvRenderParam {
    pub kind: c_int,
    pub data: *mut c_void,
}

// ---------------------------------------------------------------------------
// 常量（照抄 mpv/client.h 与 mpv/render.h）
// ---------------------------------------------------------------------------

pub mod event {
    pub const NONE: i32 = 0;
    pub const SHUTDOWN: i32 = 1;
    pub const LOG_MESSAGE: i32 = 2;
    pub const START_FILE: i32 = 6;
    pub const END_FILE: i32 = 7;
    pub const FILE_LOADED: i32 = 8;
    pub const VIDEO_RECONFIG: i32 = 17;
    pub const AUDIO_RECONFIG: i32 = 18;
    pub const SEEK: i32 = 20;
    pub const PLAYBACK_RESTART: i32 = 21;
    pub const PROPERTY_CHANGE: i32 = 22;
}

pub mod format {
    pub const NONE: i32 = 0;
    pub const STRING: i32 = 1;
    pub const FLAG: i32 = 3;
    pub const INT64: i32 = 4;
    pub const DOUBLE: i32 = 5;
}

pub mod end_file {
    pub const EOF: i32 = 0;
    pub const STOP: i32 = 2;
    pub const QUIT: i32 = 3;
    pub const ERROR: i32 = 4;
}

pub mod render_param {
    pub const INVALID: i32 = 0;
    pub const API_TYPE: i32 = 1;
    pub const ADVANCED_CONTROL: i32 = 10;
    pub const SW_SIZE: i32 = 17;
    pub const SW_FORMAT: i32 = 18;
    pub const SW_STRIDE: i32 = 19;
    pub const SW_POINTER: i32 = 20;
}

/// `mpv_render_context_update()` 的返回值：有新帧可渲染。
pub const RENDER_UPDATE_FRAME: u64 = 1;

/// 软件渲染的 API 类型字符串，配合 `MPV_RENDER_PARAM_API_TYPE` 使用。
pub const RENDER_API_TYPE_SW: &[u8] = b"sw\0";

// ---------------------------------------------------------------------------
// 函数指针表
// ---------------------------------------------------------------------------

type FnCreate = unsafe extern "C" fn() -> *mut MpvHandle;
type FnInitialize = unsafe extern "C" fn(*mut MpvHandle) -> c_int;
type FnTerminateDestroy = unsafe extern "C" fn(*mut MpvHandle);
type FnSetOptionString =
    unsafe extern "C" fn(*mut MpvHandle, *const c_char, *const c_char) -> c_int;
type FnSetProperty =
    unsafe extern "C" fn(*mut MpvHandle, *const c_char, c_int, *mut c_void) -> c_int;
type FnGetProperty =
    unsafe extern "C" fn(*mut MpvHandle, *const c_char, c_int, *mut c_void) -> c_int;
type FnGetPropertyString = unsafe extern "C" fn(*mut MpvHandle, *const c_char) -> *mut c_char;
type FnSetPropertyString =
    unsafe extern "C" fn(*mut MpvHandle, *const c_char, *const c_char) -> c_int;
type FnCommand = unsafe extern "C" fn(*mut MpvHandle, *const *const c_char) -> c_int;
type FnObserveProperty = unsafe extern "C" fn(*mut MpvHandle, u64, *const c_char, c_int) -> c_int;
type FnWaitEvent = unsafe extern "C" fn(*mut MpvHandle, c_double) -> *mut MpvEvent;
type FnErrorString = unsafe extern "C" fn(c_int) -> *const c_char;
type FnFree = unsafe extern "C" fn(*mut c_void);
type FnWakeup = unsafe extern "C" fn(*mut MpvHandle);
type FnRequestLogMessages = unsafe extern "C" fn(*mut MpvHandle, *const c_char) -> c_int;
type FnClientApiVersion = unsafe extern "C" fn() -> c_ulong;

type FnRenderContextCreate =
    unsafe extern "C" fn(*mut *mut MpvRenderContext, *mut MpvHandle, *mut MpvRenderParam) -> c_int;
type FnRenderContextSetUpdateCallback = unsafe extern "C" fn(
    *mut MpvRenderContext,
    Option<unsafe extern "C" fn(*mut c_void)>,
    *mut c_void,
);
type FnRenderContextUpdate = unsafe extern "C" fn(*mut MpvRenderContext) -> u64;
type FnRenderContextRender =
    unsafe extern "C" fn(*mut MpvRenderContext, *mut MpvRenderParam) -> c_int;
type FnRenderContextReportSwap = unsafe extern "C" fn(*mut MpvRenderContext);
type FnRenderContextFree = unsafe extern "C" fn(*mut MpvRenderContext);

/// 加载好的 libmpv。
///
/// `_library` 必须比所有函数指针活得久，所以它作为字段留在这里；结构体本身用
/// `Arc` 共享，进程存活期间不卸载。
pub struct MpvApi {
    _library: Library,
    _path: PathBuf,

    pub create: FnCreate,
    pub initialize: FnInitialize,
    pub terminate_destroy: FnTerminateDestroy,
    pub set_option_string: FnSetOptionString,
    pub set_property: FnSetProperty,
    pub get_property: FnGetProperty,
    pub get_property_string: FnGetPropertyString,
    pub set_property_string: FnSetPropertyString,
    pub command: FnCommand,
    pub observe_property: FnObserveProperty,
    pub wait_event: FnWaitEvent,
    pub error_string: FnErrorString,
    pub free: FnFree,
    pub wakeup: FnWakeup,
    pub request_log_messages: FnRequestLogMessages,
    pub client_api_version: FnClientApiVersion,

    pub render_context_create: FnRenderContextCreate,
    pub render_context_set_update_callback: FnRenderContextSetUpdateCallback,
    pub render_context_update: FnRenderContextUpdate,
    pub render_context_render: FnRenderContextRender,
    pub render_context_report_swap: FnRenderContextReportSwap,
    pub render_context_free: FnRenderContextFree,
}

// 函数指针与 Library 都可以跨线程使用；mpv 的所有 API 除 `mpv_wait_event`
// 之外都允许从不同线程调用（事件队列只有一个消费者，我们在引擎线程里消费）。
unsafe impl Send for MpvApi {}
unsafe impl Sync for MpvApi {}

impl std::fmt::Debug for MpvApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MpvApi").field("path", &self._path).finish()
    }
}

impl MpvApi {
    /// 找库并加载。
    ///
    /// 搜索顺序是有讲究的：先看用户显式指定，再看程序自身目录（免安装分发时库就放在
    /// 旁边），然后才交给系统加载器。这样同一个 exe 既能「绿色版自带 dll」，也能
    /// 「用系统包管理器装的 libmpv」。
    ///
    /// 磁盘上全都找不到时，才动用编译期内嵌的那一份——旁边或系统里已经有库就优先
    /// 用它们，用户特意换过的版本不该被内置的旧库盖掉。
    pub fn load() -> Result<Arc<Self>> {
        let mut tried = Vec::new();

        for candidate in search_paths() {
            if let Some(api) = Self::try_open(&candidate, &mut tried) {
                return Ok(api);
            }
        }

        match embed::materialize() {
            None => {}
            Some(Err(error)) => tried.push(format!("内置 libmpv 落盘失败: {error:#}")),
            Some(Ok(path)) => {
                if let Some(api) = Self::try_open(&path, &mut tried) {
                    return Ok(api);
                }
            }
        }

        bail!(
            "找不到可用的 libmpv。\n\n已尝试：\n{}\n\n\
             解决办法（任选其一）：\n\
             • 把 libmpv 放到程序目录（Windows: libmpv-2.dll；macOS: libmpv.2.dylib；Linux: libmpv.so.2）\n\
             • 用系统包管理器安装：macOS `brew install mpv`，Debian/Ubuntu `apt install libmpv2`，Fedora `dnf install mpv-libs`\n\
             • 设置环境变量 NGY_MPV_LIB 指向库文件\n\
             • 想做出「一个 exe 就能播」的版本：先运行 scripts/fetch-libmpv.sh，再 cargo build --release（构建时会把库内嵌进去）",
            tried.join("\n")
        )
    }

    /// 试一个候选路径，失败原因记进 `tried` 供出错时展示。
    fn try_open(path: &Path, tried: &mut Vec<String>) -> Option<Arc<Self>> {
        match unsafe { Self::open(path) } {
            Ok(api) => {
                log_info!(
                    "已加载 libmpv: {} ({})",
                    path.display(),
                    api.version_string()
                );
                Some(Arc::new(api))
            }
            Err(error) => {
                tried.push(format!("{}: {error:#}", path.display()));
                None
            }
        }
    }

    unsafe fn open(path: &Path) -> Result<Self> {
        unsafe {
            let library =
                Library::new(path).with_context(|| format!("加载 {} 失败", path.display()))?;

            macro_rules! sym {
                ($name:literal) => {{
                    let symbol: libloading::Symbol<'_, _> =
                        library.get($name).with_context(|| {
                            format!(
                                "{} 缺少符号 {}",
                                path.display(),
                                String::from_utf8_lossy($name)
                            )
                        })?;
                    *symbol
                }};
            }

            // 先确认版本：0.35 之前没有 render API 中的软件渲染，直接拒绝比运行到一半
            // 才发现好。
            let version_fn: FnClientApiVersion = sym!(b"mpv_client_api_version\0");
            let api_version = version_fn();
            let major = api_version >> 16;
            if major < 2 {
                bail!(
                    "libmpv 版本过旧（client API {}.{}），需要至少 2.0（对应 mpv 0.35）",
                    major,
                    api_version & 0xFFFF
                );
            }

            Ok(Self {
                create: sym!(b"mpv_create\0"),
                initialize: sym!(b"mpv_initialize\0"),
                terminate_destroy: sym!(b"mpv_terminate_destroy\0"),
                set_option_string: sym!(b"mpv_set_option_string\0"),
                set_property: sym!(b"mpv_set_property\0"),
                get_property: sym!(b"mpv_get_property\0"),
                get_property_string: sym!(b"mpv_get_property_string\0"),
                set_property_string: sym!(b"mpv_set_property_string\0"),
                command: sym!(b"mpv_command\0"),
                observe_property: sym!(b"mpv_observe_property\0"),
                wait_event: sym!(b"mpv_wait_event\0"),
                error_string: sym!(b"mpv_error_string\0"),
                free: sym!(b"mpv_free\0"),
                wakeup: sym!(b"mpv_wakeup\0"),
                request_log_messages: sym!(b"mpv_request_log_messages\0"),
                client_api_version: version_fn,
                render_context_create: sym!(b"mpv_render_context_create\0"),
                render_context_set_update_callback: sym!(
                    b"mpv_render_context_set_update_callback\0"
                ),
                render_context_update: sym!(b"mpv_render_context_update\0"),
                render_context_render: sym!(b"mpv_render_context_render\0"),
                render_context_report_swap: sym!(b"mpv_render_context_report_swap\0"),
                render_context_free: sym!(b"mpv_render_context_free\0"),
                _library: library,
                _path: path.to_path_buf(),
            })
        }
    }

    fn version_string(&self) -> String {
        let raw = unsafe { (self.client_api_version)() };
        format!("client api {}.{}", raw >> 16, raw & 0xFFFF)
    }

    /// 把 mpv 的错误码翻成人话。
    pub fn error_text(&self, code: c_int) -> String {
        if code >= 0 {
            return format!("ok({code})");
        }
        unsafe {
            let ptr = (self.error_string)(code);
            if ptr.is_null() {
                format!("error {code}")
            } else {
                CStr::from_ptr(ptr).to_string_lossy().into_owned()
            }
        }
    }

    /// 读一个字符串属性。`None` 表示属性当前不存在（mpv 返回 UNKNOWN_FORMAT 等）。
    pub fn property_string(&self, handle: *mut MpvHandle, name: &str) -> Option<String> {
        let name = CString::new(name).ok()?;
        unsafe {
            let raw = (self.get_property_string)(handle, name.as_ptr());
            if raw.is_null() {
                return None;
            }
            let value = CStr::from_ptr(raw).to_string_lossy().into_owned();
            (self.free)(raw as *mut c_void);
            Some(value)
        }
    }

    pub fn property_f64(&self, handle: *mut MpvHandle, name: &str) -> Option<f64> {
        let name = CString::new(name).ok()?;
        let mut value: c_double = 0.0;
        unsafe {
            let rc = (self.get_property)(
                handle,
                name.as_ptr(),
                format::DOUBLE,
                &mut value as *mut c_double as *mut c_void,
            );
            (rc >= 0).then_some(value)
        }
    }

    pub fn property_i64(&self, handle: *mut MpvHandle, name: &str) -> Option<i64> {
        let name = CString::new(name).ok()?;
        let mut value: i64 = 0;
        unsafe {
            let rc = (self.get_property)(
                handle,
                name.as_ptr(),
                format::INT64,
                &mut value as *mut i64 as *mut c_void,
            );
            (rc >= 0).then_some(value)
        }
    }

    pub fn property_flag(&self, handle: *mut MpvHandle, name: &str) -> Option<bool> {
        let name = CString::new(name).ok()?;
        let mut value: c_int = 0;
        unsafe {
            let rc = (self.get_property)(
                handle,
                name.as_ptr(),
                format::FLAG,
                &mut value as *mut c_int as *mut c_void,
            );
            (rc >= 0).then_some(value != 0)
        }
    }

    pub fn set_flag(&self, handle: *mut MpvHandle, name: &str, value: bool) -> Result<()> {
        let name = CString::new(name)?;
        let mut flag: c_int = i32::from(value);
        unsafe {
            let rc = (self.set_property)(
                handle,
                name.as_ptr(),
                format::FLAG,
                &mut flag as *mut c_int as *mut c_void,
            );
            if rc < 0 {
                bail!("设置 {name:?} 失败: {}", self.error_text(rc));
            }
        }
        Ok(())
    }

    pub fn set_f64(&self, handle: *mut MpvHandle, name: &str, value: f64) -> Result<()> {
        let name = CString::new(name)?;
        let mut value = value;
        unsafe {
            let rc = (self.set_property)(
                handle,
                name.as_ptr(),
                format::DOUBLE,
                &mut value as *mut c_double as *mut c_void,
            );
            if rc < 0 {
                bail!("设置 {name:?} 失败: {}", self.error_text(rc));
            }
        }
        Ok(())
    }

    pub fn set_string(&self, handle: *mut MpvHandle, name: &str, value: &str) -> Result<()> {
        let name = CString::new(name)?;
        let value = CString::new(value)?;
        unsafe {
            let rc = (self.set_property_string)(handle, name.as_ptr(), value.as_ptr());
            if rc < 0 {
                bail!("设置 {name:?} 失败: {}", self.error_text(rc));
            }
        }
        Ok(())
    }

    /// 执行 mpv 命令，例如 `loadfile`、`seek`、`cycle`。
    pub fn run_command(&self, handle: *mut MpvHandle, args: &[&str]) -> Result<()> {
        let owned: Vec<CString> = args
            .iter()
            .map(|arg| CString::new(*arg))
            .collect::<Result<_, _>>()?;
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|arg| arg.as_ptr()).collect();
        ptrs.push(std::ptr::null());

        unsafe {
            let rc = (self.command)(handle, ptrs.as_ptr());
            if rc < 0 {
                bail!("mpv 命令 {:?} 失败: {}", args, self.error_text(rc));
            }
        }
        Ok(())
    }

    pub fn observe(&self, handle: *mut MpvHandle, id: u64, name: &str, fmt: i32) -> Result<()> {
        let name = CString::new(name)?;
        unsafe {
            let rc = (self.observe_property)(handle, id, name.as_ptr(), fmt);
            if rc < 0 {
                bail!("监听属性 {name:?} 失败: {}", self.error_text(rc));
            }
        }
        Ok(())
    }
}

/// 按优先级列出所有值得一试的库路径。
fn search_paths() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(explicit) = std::env::var("NGY_MPV_LIB") {
        if !explicit.is_empty() {
            candidates.push(PathBuf::from(explicit));
        }
    }

    let names: &[&str] = if cfg!(windows) {
        &["libmpv-2.dll", "libmpv.dll", "mpv-2.dll"]
    } else if cfg!(target_os = "macos") {
        &["libmpv.2.dylib", "libmpv.dylib"]
    } else {
        &["libmpv.so.2", "libmpv.so.1", "libmpv.so"]
    };

    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.to_path_buf());
            // macOS .app 布局：可执行文件在 Contents/MacOS，库在 Contents/Frameworks。
            if let Some(contents) = dir.parent() {
                dirs.push(contents.join("Frameworks"));
                dirs.push(contents.join("Resources"));
            }
            // 开发期：target/debug/../../libs/<platform>
            if let Some(root) = dir.parent().and_then(|p| p.parent()) {
                dirs.push(root.join("libs").join(platform_dir()));
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("libs").join(platform_dir()));
        dirs.push(cwd);
    }

    for dir in dirs {
        for name in names {
            candidates.push(dir.join(name));
        }
    }

    // 交给系统加载器（PATH / LD_LIBRARY_PATH / dyld 搜索路径）。
    candidates.extend(names.iter().map(PathBuf::from));

    // macOS 上 Homebrew 的前缀不在 dyld 默认搜索路径里的情况很常见。
    #[cfg(target_os = "macos")]
    {
        for prefix in ["/opt/homebrew/lib", "/usr/local/lib"] {
            for name in names {
                candidates.push(Path::new(prefix).join(name));
            }
        }
    }

    // Linux 多架构目录。
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for prefix in [
            "/usr/lib/x86_64-linux-gnu",
            "/usr/lib/aarch64-linux-gnu",
            "/usr/lib64",
            "/usr/lib",
            "/usr/local/lib",
        ] {
            for name in names {
                candidates.push(Path::new(prefix).join(name));
            }
        }
    }

    candidates
}

pub fn platform_dir() -> &'static str {
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

/// 打开 libmpv 日志：只在需要排查解码问题时开启（NGY_MPV_VERBOSE=1）。
pub fn verbose_logging() -> bool {
    std::env::var("NGY_MPV_VERBOSE")
        .map(|value| value != "0" && !value.is_empty())
        .unwrap_or(false)
}

/// 让一个裸指针可以跨线程传递。调用方必须保证：指针指向的 mpv 对象在接收线程使用
/// 期间一直有效，且不并发销毁。
pub struct SendPtr<T>(pub *mut T);
unsafe impl<T> Send for SendPtr<T> {}
unsafe impl<T> Sync for SendPtr<T> {}
impl<T> Copy for SendPtr<T> {}
impl<T> Clone for SendPtr<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> SendPtr<T> {
    pub fn get(self) -> *mut T {
        self.0
    }
}

/// 从 mpv 返回的 JSON 属性里取字符串字段的小工具。
pub fn json_str(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

pub fn json_i64(value: &serde_json::Value, key: &str) -> Option<i64> {
    value.get(key).and_then(|v| v.as_i64())
}

pub fn json_f64(value: &serde_json::Value, key: &str) -> Option<f64> {
    value.get(key).and_then(|v| v.as_f64())
}

/// mpv 的 `track-list` 是 JSON 数组；解析失败时返回空列表而不是报错——
/// 轨道信息缺失不影响播放。
pub fn parse_track_list(raw: &str) -> Vec<serde_json::Value> {
    serde_json::from_str::<Vec<serde_json::Value>>(raw).unwrap_or_default()
}

/// 构造一个以 `name=value` 形式传选项的辅助函数所需的 `CString`。
pub fn cstring(value: &str) -> Result<CString> {
    CString::new(value).map_err(|_| anyhow!("字符串包含 NUL 字节：{value:?}"))
}
