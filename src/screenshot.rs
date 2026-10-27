//! 截图的落盘位置与文件名。
//!
//! 抓帧本身交给 mpv（`screenshot-to-file`），这里只管两件事：存到哪儿、叫什么。
//! 文件名用「视频名 + 播放时间」，同一部片子连拍多张也能看出各自对应哪一刻。

use anyhow::{Context as _, Result, anyhow};
use std::path::{Path, PathBuf};

/// 截图目录：系统图片目录下的 `影匣截图/`。
fn pictures_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from)?;
        Some(home.join("Pictures").join("影匣截图"))
    }

    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        Some(home.join("Pictures").join("影匣截图"))
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let base = std::env::var_os("XDG_PICTURES_DIR")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join("Pictures"))
            })?;
        Some(base.join("影匣截图"))
    }
}

/// 为一次截图挑一个不会覆盖已有文件的目标路径。
///
/// 系统图片目录不可用时退回视频所在目录：用户至少知道东西存在哪儿，
/// 而不是弹一句「保存失败」就没了下文。
pub fn target(video: Option<&Path>, position: f64) -> Result<PathBuf> {
    let directory = pictures_dir()
        .or_else(|| video.and_then(Path::parent).map(Path::to_path_buf))
        .ok_or_else(|| anyhow!("找不到可用的截图目录"))?;
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("创建截图目录失败：{}", directory.display()))?;

    let stem = video
        .and_then(Path::file_stem)
        .and_then(|stem| stem.to_str())
        .map(str::to_owned)
        .map(|stem| sanitize(&stem))
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "截图".to_string());
    let base = format!("{stem}_{}", timecode(position));

    let mut candidate = directory.join(format!("{base}.png"));
    let mut serial = 2;
    while candidate.exists() {
        candidate = directory.join(format!("{base}-{serial}.png"));
        serial += 1;
    }
    Ok(candidate)
}

/// 去掉文件名里各平台不接受的字符。
///
/// 按 Windows 最严格的一套来：其它平台跟着一起换掉，同一部片子在三处产出的
/// 文件名才会一致，跨平台同步盘里也不会突然出现非法名。
fn sanitize(stem: &str) -> String {
    let cleaned: String = stem
        .chars()
        .map(|ch| {
            if ch.is_control() || "<>:\"/\\|?*".contains(ch) {
                '_'
            } else {
                ch
            }
        })
        .collect();
    // Windows 不允许名字以点或空格结尾；过长的名字也留点余量给时间戳和扩展名。
    cleaned
        .trim()
        .trim_end_matches('.')
        .chars()
        .take(60)
        .collect::<String>()
        .trim()
        .to_string()
}

/// 播放位置 → `01-23-45`。冒号在各平台上都不能进文件名，这里直接用连字符。
fn timecode(position: f64) -> String {
    let total = position.max(0.0).floor() as u64;
    format!(
        "{:02}-{:02}-{:02}",
        total / 3600,
        total % 3600 / 60,
        total % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 非法字符替换成下划线() {
        assert_eq!(sanitize("第1集: 开头/结尾?"), "第1集_ 开头_结尾_");
        assert_eq!(sanitize("a\\b|c*d\"e<f>g"), "a_b_c_d_e_f_g");
    }

    #[test]
    fn 空白与结尾的点会被清理() {
        assert_eq!(sanitize("  片尾.  "), "片尾");
        assert_eq!(sanitize("   "), "");
    }

    #[test]
    fn 过长的名字会截断() {
        let long = "长".repeat(200);
        assert_eq!(sanitize(&long).chars().count(), 60);
    }

    #[test]
    fn 时间戳按小时分钟秒补零() {
        assert_eq!(timecode(0.0), "00-00-00");
        assert_eq!(timecode(59.9), "00-00-59");
        assert_eq!(timecode(3725.0), "01-02-05");
    }
}
