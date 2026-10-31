//! 文件夹递归扫描：把一棵目录树变成一串待转换的文件。

use crate::formats;
use crate::playlist::natural_cmp;
use anyhow::Context as _;
use std::path::{Path, PathBuf};

/// 递归深度上限。符号链接已经不跟了，这里只是防止一棵离谱深的目录树把界面拖死。
const MAX_DEPTH: usize = 32;

/// 递归收集 `root` 下所有能播的媒体，按「目录 + 自然序文件名」排好。
///
/// 同步执行：调用方负责放到后台线程上（几百个文件的目录可能要读几秒）。
///
/// 只有**根目录**读不了才算失败——用户会想立刻知道选错了目录。子目录读不了就
/// 跳过：一批几十个文件不该因为某个深层目录没权限而整批放弃。
pub fn scan_folder(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let entries =
        std::fs::read_dir(root).with_context(|| format!("读不了目录：{}", root.display()))?;
    let mut files = Vec::new();
    collect(entries, 0, &mut files);
    files.sort_by(|a, b| {
        // 先按目录分组，同目录里再按自然序：`第2集` 排在 `第10集` 前面，
        // 子文件夹里的文件也不会和父目录里的交错在一起。
        a.parent().cmp(&b.parent()).then_with(|| natural_cmp(a, b))
    });
    Ok(files)
}

fn collect(entries: std::fs::ReadDir, depth: usize, files: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH {
        return;
    }
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            // 不跟符号链接：环形链接会让扫描停不下来。
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            if let Ok(next) = std::fs::read_dir(&path) {
                collect(next, depth + 1, files);
            }
        } else if formats::is_media(&path) {
            files.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ngy-convert-scan-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn 扫描包含子文件夹且跳过非媒体文件() {
        let dir = temp_dir("tree");
        std::fs::create_dir_all(dir.join("第二讲/素材")).unwrap();
        std::fs::write(dir.join("第10集.mkv"), b"x").unwrap();
        std::fs::write(dir.join("第2集.mp4"), b"x").unwrap();
        std::fs::write(dir.join("说明.txt"), b"x").unwrap();
        std::fs::write(dir.join("第二讲/第1集.mp4"), b"x").unwrap();
        std::fs::write(dir.join("第二讲/素材/花絮.mkv"), b"x").unwrap();

        let files = scan_folder(&dir).unwrap();
        let names: Vec<String> = files
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec!["第2集.mp4", "第10集.mkv", "第1集.mp4", "花絮.mkv"]
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn 不存在的目录是错误而不是空列表() {
        let missing = std::env::temp_dir().join("ngy-convert-scan-missing");
        let _ = std::fs::remove_dir_all(&missing);
        assert!(scan_folder(&missing).is_err());
    }
}
