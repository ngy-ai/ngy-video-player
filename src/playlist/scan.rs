//! 目录扫描与列表文件解析。
//!
//! 这里只做「把一个目录/一份 m3u 变成一串路径」的活，不关心层级：树形展开
//! 在 `tree` 里，两边共用的只有这里的自然序比较。

use crate::formats;
use std::path::{Path, PathBuf};

/// 目录里**直接**放着的媒体文件，按文件名自然序。
///
/// 刻意不递归：这条路径服务于「打开一个文件时把同目录的其它集一起排队」，
/// 顺手把子目录里的东西也拉进来会打乱用户预期的队列。递归展开是
/// [`super::collect_tree`] 的事。
pub fn collect_dir(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| formats::is_media(path))
        .collect();
    files.sort_by(|a, b| natural_cmp(a, b));
    files
}

/// 自然序比较：`第2集` 排在 `第10集` 前面。
pub(crate) fn natural_cmp(a: &Path, b: &Path) -> std::cmp::Ordering {
    let left = a
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let right = b
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    let mut left_chars = left.chars().peekable();
    let mut right_chars = right.chars().peekable();

    loop {
        match (left_chars.peek().copied(), right_chars.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(l), Some(r)) => {
                if l.is_ascii_digit() && r.is_ascii_digit() {
                    let left_number = take_number(&mut left_chars);
                    let right_number = take_number(&mut right_chars);
                    match left_number.cmp(&right_number) {
                        std::cmp::Ordering::Equal => continue,
                        other => return other,
                    }
                } else {
                    left_chars.next();
                    right_chars.next();
                    match l.cmp(&r) {
                        std::cmp::Ordering::Equal => continue,
                        other => return other,
                    }
                }
            }
        }
    }
}

fn take_number(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> u64 {
    let mut value: u64 = 0;
    while let Some(c) = chars.peek().copied() {
        if c.is_ascii_digit() {
            value = value
                .saturating_mul(10)
                .saturating_add(c as u64 - '0' as u64);
            chars.next();
        } else {
            break;
        }
    }
    value
}

/// 解析 m3u/m3u8/pls。相对路径按列表文件所在目录解析。
pub fn parse_playlist(path: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let is_pls = formats::extension(path).as_deref() == Some("pls");

    let mut result = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let candidate = if is_pls {
            // PLS 的条目形如 File1=/path/to/a.mp4
            match line.split_once('=') {
                Some((key, value)) if key.trim().to_lowercase().starts_with("file") => {
                    value.trim().to_string()
                }
                _ => continue,
            }
        } else {
            line.to_string()
        };

        if candidate.contains("://") {
            // 网络流不是文件路径，播放列表这一层先跳过（可通过命令行直接打开）。
            continue;
        }
        let resolved = if Path::new(&candidate).is_absolute() {
            PathBuf::from(&candidate)
        } else {
            base.join(&candidate)
        };
        if resolved.exists() {
            result.push(resolved);
        }
    }
    result
}
