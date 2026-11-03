//! 产物落点：裁切、删除中间段、分割、拼接、批处理各一套后缀。
//!
//! 一律与原文件同目录、名字加后缀，已存在就往后编号。不覆盖已有文件：导出是耗时
//! 操作，一次手滑把同名文件盖掉就找不回来了。HLS/DASH 只检查播放列表，同名分片
//! 交给 ffmpeg 覆盖——它们属于这一个播放列表。

use super::OutputFormat;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// 裁切结果的落点：与原文件同目录，名字加「 - 片段」。
pub fn output_path(input: &Path, format: OutputFormat) -> PathBuf {
    numbered_output(input, format, "片段")
}

/// 「删除中间段」的落点：后缀换成「删除片段」，免得和普通裁切混在一起。
pub fn cut_out_output_path(input: &Path) -> PathBuf {
    numbered_output(input, OutputFormat::Mkv, "删除片段")
}

/// 拼接的落点：放在第一个文件旁边，名字加「 - 拼接」。
pub fn concat_output_path(first: &Path) -> PathBuf {
    numbered_output(first, OutputFormat::Mkv, "拼接")
}

/// 分割的两个落点：前后两段各一个文件。
pub fn split_output_paths(input: &Path) -> (PathBuf, PathBuf) {
    (
        numbered_output(input, OutputFormat::Mkv, "段落1"),
        numbered_output(input, OutputFormat::Mkv, "段落2"),
    )
}

/// 批处理的落点：名字加「 - 批处理」，与单文件裁切的产物区分开。
///
/// `taken` 是这一批里已经分出去的落点：同一个目录下的 `a.mkv` 与 `a.mp4` 会算出
/// 同一个名字，只看「文件是否存在」挡不住。
pub fn batch_output_path(input: &Path, format: OutputFormat, taken: &HashSet<PathBuf>) -> PathBuf {
    numbered_output_with(input, format, "批处理", |path| taken.contains(path))
}

/// 拼 `原名 - 后缀.ext`，撞名时加 `(2)`、`(3)`。
pub(super) fn numbered_output(input: &Path, format: OutputFormat, suffix: &str) -> PathBuf {
    numbered_output_with(input, format, suffix, |_| false)
}

/// 同上，`taken` 用来挡掉「还没落盘但已经许给别人」的名字。
fn numbered_output_with(
    input: &Path,
    format: OutputFormat,
    suffix: &str,
    taken: impl Fn(&Path) -> bool,
) -> PathBuf {
    let dir = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "片段".to_string());
    let base = format!("{stem} - {suffix}");
    let extension = format.extension();

    let mut candidate = dir.join(format!("{base}.{extension}"));
    let mut index = 2;
    while candidate.exists() || taken(&candidate) {
        candidate = dir.join(format!("{base} ({index}).{extension}"));
        index += 1;
        // 理论上不会走到这里；只是防止某个坏掉的目录让循环停不下来。
        if index > 999 {
            break;
        }
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 输出文件不覆盖已有文件() {
        let dir = std::env::temp_dir().join(format!("ngy-clip-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("课程 01.mp4");
        std::fs::write(&input, b"x").unwrap();

        let first = output_path(&input, OutputFormat::Mkv);
        assert_eq!(first, dir.join("课程 01 - 片段.mkv"));
        std::fs::write(&first, b"x").unwrap();

        let second = output_path(&input, OutputFormat::Mkv);
        assert_eq!(second, dir.join("课程 01 - 片段 (2).mkv"));

        // HLS 只认自己的播放列表，不会被已有的 mkv 影响。
        let hls = output_path(&input, OutputFormat::Hls);
        assert_eq!(hls, dir.join("课程 01 - 片段.m3u8"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn 同一批里重名的落点错开() {
        let dir = std::env::temp_dir().join(format!("ngy-batch-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 同一目录下的同名不同扩展：两者的「 - 批处理.mkv」是同一个名字。
        let mkv = dir.join("片子.mkv");
        let mp4 = dir.join("片子.mp4");

        let mut taken = HashSet::new();
        let first = batch_output_path(&mkv, OutputFormat::Mkv, &taken);
        taken.insert(first.clone());
        let second = batch_output_path(&mp4, OutputFormat::Mkv, &taken);

        assert_eq!(first, dir.join("片子 - 批处理.mkv"));
        assert_eq!(second, dir.join("片子 - 批处理 (2).mkv"));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
