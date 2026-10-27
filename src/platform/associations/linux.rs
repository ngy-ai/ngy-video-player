//! Linux 上的文件关联。
//!
//! Linux 没有「请用户去设置页点一下」这一步：把 `.desktop` 写进
//! `$XDG_DATA_HOME/applications`，再把自己的条目填进 `mimeapps.list` 的
//! `[Default Applications]`，关联当场生效。
//!
//! 不调用 `xdg-mime` / `update-desktop-database`：前者只是这个文件的写入器，后者
//! 是为老桌面环境准备的缓存刷新，现代桌面环境靠文件 mtime 自己发现变化。少依赖两个
//! 不一定装了的命令，行为还更可预测。

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use super::{Status, mimeapps};

pub const SUPPORTED: bool = true;

const APP_NAME: &str = "ngy-video-player";
const DESKTOP_ID: &str = "ngy-video-player.desktop";
const ICON_SIZE: u32 = 256;
/// 图标直接复用 Windows 那份 ico，运行时转成桌面环境认识的 PNG——
/// 仓库里不必为同一个图标存第三份文件。
const ICON_BYTES: &[u8] = include_bytes!("../../../assets/icon.ico");

fn data_home() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".local/share"))
}

fn config_home() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".config"))
}

fn desktop_path() -> Option<PathBuf> {
    Some(data_home()?.join("applications").join(DESKTOP_ID))
}

fn icon_path() -> Option<PathBuf> {
    Some(
        data_home()?
            .join("icons/hicolor")
            .join(format!("{ICON_SIZE}x{ICON_SIZE}"))
            .join("apps")
            .join(format!("{APP_NAME}.png")),
    )
}

fn mimeapps_path() -> Option<PathBuf> {
    Some(config_home()?.join("mimeapps.list"))
}

/// `shared-mime-info` 的数据库。`globs2` 是新格式，`globs` 是给老系统留的后路。
fn shared_mime_globs() -> Option<String> {
    ["/usr/share/mime/globs2", "/usr/share/mime/globs"]
        .iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
}

fn write_file(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建目录 {} 失败", parent.display()))?;
    }
    std::fs::write(path, contents).with_context(|| format!("写 {} 失败", path.display()))
}

fn write_icon(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    let icon = image::load_from_memory_with_format(ICON_BYTES, image::ImageFormat::Ico)
        .context("内置图标解码失败")?;
    // ico 里最大的一层提出来按标准尺寸落盘；hicolor 主题靠文件名判断尺寸。
    let icon = icon.resize_exact(ICON_SIZE, ICON_SIZE, image::imageops::FilterType::Lanczos3);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建目录 {} 失败", parent.display()))?;
    }
    icon.save(path)
        .with_context(|| format!("写图标 {} 失败", path.display()))
}

pub fn register() -> Result<Status> {
    let exe = std::env::current_exe().context("取不到当前可执行文件的路径")?;
    let exe = exe.to_string_lossy().to_string();

    let mapping = mimeapps::resolve(
        shared_mime_globs().as_deref(),
        &crate::formats::associable_extensions(),
    );
    if mapping.is_empty() {
        bail!("系统里查不到这些扩展名对应的 MIME 类型，请确认已安装 shared-mime-info");
    }
    let mimes = mimeapps::unique_mimes(&mapping);

    let desktop =
        desktop_path().context("定位不到 XDG 数据目录（HOME 或 XDG_DATA_HOME 未设置）")?;
    let icon = icon_path().context("定位不到图标目录（HOME 或 XDG_DATA_HOME 未设置）")?;
    write_icon(&icon)?;

    let entry = mimeapps::desktop_entry(&exe, &icon.to_string_lossy(), &mimes);
    write_file(&desktop, &entry)?;

    if let Some(path) = mimeapps_path() {
        let original = std::fs::read_to_string(&path).unwrap_or_default();
        let updated = mimeapps::set_defaults(&original, DESKTOP_ID, &mimes);
        write_file(&path, &updated)?;
    }

    Ok(Status {
        registered: mapping.len(),
        target: Some(PathBuf::from(exe)),
    })
}

pub fn status() -> Result<Status> {
    let Some(desktop) = desktop_path() else {
        return Ok(Status::default());
    };
    let Ok(text) = std::fs::read_to_string(&desktop) else {
        return Ok(Status::default());
    };

    let (executable, mimes) = mimeapps::parse_desktop(&text);
    let mapping = mimeapps::resolve(
        shared_mime_globs().as_deref(),
        &crate::formats::associable_extensions(),
    );
    // 按「扩展名」计数，和 Windows 那边口径一致：多个扩展名共用同一个 MIME
    // （ts/m2ts 都是 video/mp2t）时，不能因为去重把数字算少。
    let registered = mapping
        .values()
        .filter(|mime| mimes.iter().any(|item| item == *mime))
        .count();

    Ok(Status {
        registered,
        target: executable.map(PathBuf::from),
    })
}

pub fn remove() -> Result<()> {
    if let Some(path) = desktop_path() {
        let _ = std::fs::remove_file(&path);
    }
    if let Some(path) = icon_path() {
        let _ = std::fs::remove_file(&path);
    }
    if let Some(path) = mimeapps_path() {
        if let Ok(original) = std::fs::read_to_string(&path) {
            let cleaned = mimeapps::remove_defaults(&original, DESKTOP_ID);
            write_file(&path, &cleaned)?;
        }
    }
    Ok(())
}

/// Linux 上注册即生效，没有需要用户再确认的页面。
pub fn open_system_settings() -> Result<()> {
    Ok(())
}

pub fn guidance() -> &'static str {
    "已经直接成为这些格式的默认播放器。"
}
