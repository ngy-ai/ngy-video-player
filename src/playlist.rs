//! 播放列表。
//!
//! 刻意不把列表交给 mpv 的 `playlist-*` 命令：那套状态在切歌、拖放、手动重排时
//! 会和界面各记一份，两边一旦不同步就很难查。这里 UI 是唯一事实来源，mpv 每次只
//! 负责「放这一个文件」。
//!
//! 面板上可以有多个列表（一页一个），整组列表连同各自的位置一起存盘，
//! 下次启动原样恢复——「记住上次在听哪张专辑」比记住窗口大小更贴近使用习惯。

use crate::formats;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Playlist {
    items: Vec<PathBuf>,
    index: Option<usize>,
}

/// 删除若干项之后的结果。界面据此决定是换歌、停播，还是什么都不用做。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Removal {
    /// 实际删掉的项数：越界索引和重复索引不计。
    pub removed: usize,
    /// 被删掉的项里包含当时正在播的那一项。
    pub current_removed: bool,
    /// 删除后应当看作当前项的索引；`None` 表示列表已经空了。
    pub current: Option<usize>,
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

    /// 删除一项，等价于 `remove_many(&[index])`。
    pub fn remove(&mut self, index: usize) -> Removal {
        self.remove_many(&[index])
    }

    /// 批量删除。`indexes` 用的是**删除前**的索引。
    ///
    /// 当前项被删掉时不是简单地停住：它落到后面第一个幸存者，后面没有了就退回
    /// 前面最后一个——和用户点「下一集」的直觉一致。列表被删空时不留当前项。
    pub fn remove_many(&mut self, indexes: &[usize]) -> Removal {
        let mut targets: Vec<usize> = indexes
            .iter()
            .copied()
            .filter(|index| *index < self.items.len())
            .collect();
        targets.sort_unstable();
        targets.dedup();
        if targets.is_empty() {
            return Removal {
                removed: 0,
                current_removed: false,
                current: self.index,
            };
        }

        let current = self.index;
        let current_removed = current.is_some_and(|value| targets.binary_search(&value).is_ok());
        // 先按旧索引挑好接班人，再动列表：删完之后所有下标都变了，那时再找会错位。
        let successor = current.and_then(|value| {
            (value..self.items.len())
                .find(|index| targets.binary_search(index).is_err())
                .or_else(|| {
                    (0..value)
                        .rev()
                        .find(|index| targets.binary_search(index).is_err())
                })
        });

        for index in targets.iter().rev() {
            self.items.remove(*index);
        }

        let shift = |index: usize| index - targets.iter().filter(|target| **target < index).count();
        self.index = if current_removed {
            successor.map(shift)
        } else {
            current.map(shift)
        }
        .filter(|index| *index < self.items.len());

        Removal {
            removed: targets.len(),
            current_removed,
            current: self.index,
        }
    }

    /// 清空列表。只丢列表本身，磁盘上的文件一动不动。
    pub fn clear(&mut self) {
        self.items.clear();
        self.index = None;
    }

    /// 从存档读回来之后校正：索引可能指向已经不存在的项。
    fn sanitize(&mut self) {
        self.index = self.index.filter(|index| *index < self.items.len());
    }
}

/// 面板上的一页：一个名字 + 一份列表。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistTab {
    pub name: String,
    pub playlist: Playlist,
}

/// 全部列表 + 当前显示的是哪一页。
///
/// 不变量：至少有一页。删到最后一页时不再允许删，保证界面永远有个落脚点。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistSet {
    tabs: Vec<PlaylistTab>,
    active: usize,
}

/// `active_playlist()` 在极端情况下（空集合）要能返回一个只读的占位。
static EMPTY_PLAYLIST: Playlist = Playlist {
    items: Vec::new(),
    index: None,
};

impl Default for PlaylistSet {
    fn default() -> Self {
        Self {
            tabs: vec![PlaylistTab {
                name: "列表 1".to_string(),
                playlist: Playlist::default(),
            }],
            active: 0,
        }
    }
}

impl PlaylistSet {
    pub fn tabs(&self) -> &[PlaylistTab] {
        &self.tabs
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    /// 一页都没有。正常路径下不会出现（至少保留一页），留给调用方做防御。
    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn active(&self) -> usize {
        self.active.min(self.tabs.len().saturating_sub(1))
    }

    pub fn set_active(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
        }
    }

    pub fn tab(&self, index: usize) -> Option<&Playlist> {
        self.tabs.get(index).map(|tab| &tab.playlist)
    }

    pub fn tab_mut(&mut self, index: usize) -> Option<&mut Playlist> {
        self.tabs.get_mut(index).map(|tab| &mut tab.playlist)
    }

    pub fn active_playlist(&self) -> &Playlist {
        self.tabs
            .get(self.active())
            .map(|tab| &tab.playlist)
            .unwrap_or(&EMPTY_PLAYLIST)
    }

    pub fn active_playlist_mut(&mut self) -> &mut Playlist {
        if self.tabs.is_empty() {
            self.tabs.push(PlaylistTab::default());
        }
        let index = self.active();
        &mut self.tabs[index].playlist
    }

    pub fn rename_active(&mut self, name: impl Into<String>) {
        let index = self.active();
        if let Some(tab) = self.tabs.get_mut(index) {
            tab.name = name.into();
        }
    }

    /// 新建一页并切过去，返回新页的下标。
    pub fn add(&mut self, name: impl Into<String>) -> usize {
        self.tabs.push(PlaylistTab {
            name: name.into(),
            playlist: Playlist::default(),
        });
        self.active = self.tabs.len() - 1;
        self.active
    }

    /// 关掉一页。只剩一页时拒绝，返回是否真的关了。
    pub fn remove(&mut self, index: usize) -> bool {
        if self.tabs.len() <= 1 || index >= self.tabs.len() {
            return false;
        }
        self.tabs.remove(index);
        if self.active > index {
            self.active -= 1;
        } else if self.active == index {
            // 关掉的是当前页：落到右边那一页，没有右边就落回最后一页。
            self.active = index.min(self.tabs.len() - 1);
        }
        true
    }

    /// 没用过的名字 `列表 N`，跳过已经被占用的号。
    pub fn auto_name(&self) -> String {
        let mut number = self.tabs.len() + 1;
        loop {
            let name = format!("列表 {number}");
            if !self.tabs.iter().any(|tab| tab.name == name) {
                return name;
            }
            number += 1;
        }
    }

    /// 解析存档文本。坏数据一律退回默认值——启动路径上不该因为存档坏掉而起不来。
    pub fn from_json(text: &str) -> Self {
        match serde_json::from_str::<Self>(text) {
            Ok(mut set) => {
                set.sanitize();
                set
            }
            Err(error) => {
                log_warn!("播放列表存档解析失败，按空列表处理: {error}");
                Self::default()
            }
        }
    }

    pub fn load() -> Self {
        let Some(path) = crate::settings::playlists_path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::from_json(&text),
            Err(_) => Self::default(),
        }
    }

    /// 落盘。写临时文件再改名，崩溃时不会留下半截存档。
    pub fn save(&self) {
        let Some(path) = crate::settings::playlists_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let Ok(text) = serde_json::to_string_pretty(self) else {
            return;
        };
        let temp = path.with_extension("json.tmp");
        if let Err(error) = std::fs::write(&temp, text).and_then(|()| std::fs::rename(&temp, &path))
        {
            log_warn!("保存播放列表失败: {error}");
        }
    }

    fn sanitize(&mut self) {
        if self.tabs.is_empty() {
            self.tabs.push(PlaylistTab {
                name: "列表 1".to_string(),
                playlist: Playlist::default(),
            });
        }
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            if tab.name.trim().is_empty() {
                tab.name = format!("列表 {}", index + 1);
            }
            tab.playlist.sanitize();
        }
        self.active = self.active.min(self.tabs.len() - 1);
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
