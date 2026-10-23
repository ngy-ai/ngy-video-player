//! 播放列表。
//!
//! 刻意不把列表交给 mpv 的 `playlist-*` 命令：那套状态在切歌、拖放、手动重排时
//! 会和界面各记一份，两边一旦不同步就很难查。这里 UI 是唯一事实来源，mpv 每次只
//! 负责「放这一个文件」。

use crate::formats;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct Playlist {
    items: Vec<PathBuf>,
    index: Option<usize>,
}

impl Playlist {
    pub fn items(&self) -> &[PathBuf] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn index(&self) -> Option<usize> {
        self.index
    }

    pub fn current(&self) -> Option<&PathBuf> {
        self.index.and_then(|index| self.items.get(index))
    }

    pub fn get(&self, index: usize) -> Option<&PathBuf> {
        self.items.get(index)
    }

    /// 整批替换，并把 `start` 设为当前项。
    pub fn replace(&mut self, items: Vec<PathBuf>, start: usize) {
        self.items = items;
        self.index = (!self.items.is_empty()).then(|| start.min(self.items.len() - 1));
    }

    pub fn set_index(&mut self, index: usize) {
        if index < self.items.len() {
            self.index = Some(index);
        }
    }

    /// 追加文件；目录会被展开成其中可播放的文件（按文件名自然序）。
    pub fn append_paths(&mut self, paths: &[PathBuf]) {
        for path in paths {
            if path.is_dir() {
                let mut files = collect_dir(path);
                self.items.append(&mut files);
            } else if formats::is_media(path) {
                if !self.items.iter().any(|existing| existing == path) {
                    self.items.push(path.clone());
                }
            } else if formats::is_playlist(path) {
                let mut entries = parse_playlist(path);
                self.items.append(&mut entries);
            }
        }
    }

    pub fn next_index(&self, wrap: bool) -> Option<usize> {
        let current = self.index?;
        let next = current + 1;
        if next < self.items.len() {
            Some(next)
        } else if wrap && !self.items.is_empty() {
            Some(0)
        } else {
            None
        }
    }

    pub fn previous_index(&self, wrap: bool) -> Option<usize> {
        let current = self.index?;
        if current > 0 {
            Some(current - 1)
        } else if wrap && !self.items.is_empty() {
            Some(self.items.len() - 1)
        } else {
            None
        }
    }

    pub fn remove(&mut self, index: usize) {
        if index >= self.items.len() {
            return;
        }
        self.items.remove(index);
        self.index = match self.index {
            Some(current) if current > index => Some(current - 1),
            Some(current) if current == index => {
                if self.items.is_empty() {
                    None
                } else {
                    Some(current.min(self.items.len() - 1))
                }
            }
            other => other,
        };
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.index = None;
    }
}

/// 目录里所有看起来能播的文件，按文件名排序。
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
fn natural_cmp(a: &Path, b: &Path) -> std::cmp::Ordering {
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
