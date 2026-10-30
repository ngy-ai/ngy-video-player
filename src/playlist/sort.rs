//! 排序与顺序历史。
//!
//! 排序是「点一次执行一次」的动作，不是一直生效的规则：排完之后再拖进来一个文件，
//! 它一律追加到末尾——按名字插到中间会让人找不到刚放进来的那一项。当前用的是哪种
//! 排法只记下来给菜单打勾。
//!
//! 树形列表按层排：每一层的兄弟节点各自排序，文件夹排在文件前面，子层跟着父项
//! 一起走。所以排完一遍，目录结构还是原来那个目录结构。
//!
//! 每次改顺序之前，先把当时的排列存一份进历史（最多 [`HISTORY_LIMIT`] 份），
//! 排错了能一键退回去。历史存完整条目而不是下标：文件删掉之后下标会全部错位，
//! 而层级也得跟着一起回来，否则还原出来会是一张散架的树。

use super::scan::natural_cmp;
use super::{Item, Playlist};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 历史顺序最多留几份。
pub(super) const HISTORY_LIMIT: usize = 3;

/// 列表当前的排列方式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortMode {
    /// 按加入列表的先后（默认，也是列表最初的样子）。
    #[default]
    Added,
    Name,
    /// 按文件修改时间，旧的在前面。
    Modified,
    /// 用户手工拖出来的顺序。
    Manual,
}

impl SortMode {
    pub fn label(self) -> &'static str {
        match self {
            SortMode::Added => "按添加时间",
            SortMode::Name => "按名称",
            SortMode::Modified => "按修改时间",
            SortMode::Manual => "手工排序",
        }
    }

    /// 菜单里的完整说明。方向写出来，用户不用点一次才知道是升序还是降序。
    pub fn menu_label(self) -> &'static str {
        match self {
            SortMode::Added => "按添加时间（早→晚）",
            SortMode::Name => "按名称（A→Z）",
            SortMode::Modified => "按修改时间（旧→新）",
            SortMode::Manual => "手工排序",
        }
    }

    fn tag(self) -> &'static str {
        match self {
            SortMode::Added => "added",
            SortMode::Name => "name",
            SortMode::Modified => "modified",
            SortMode::Manual => "manual",
        }
    }
}

/// 存档里认不出的排法一律退回默认值，而不是让整份列表作废。
impl<'de> Deserialize<'de> for SortMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Ok([
            SortMode::Added,
            SortMode::Name,
            SortMode::Modified,
            SortMode::Manual,
        ]
        .into_iter()
        .find(|mode| mode.tag() == text)
        .unwrap_or_default())
    }
}

impl Serialize for SortMode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.tag())
    }
}

/// 一版旧顺序：改之前是按什么排的、当时都有哪些项、各自在第几层。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OrderVersion {
    mode: SortMode,
    /// 老存档只留了路径。写盘时不再输出，只用于读回旧存档。
    #[serde(skip_serializing)]
    paths: Vec<PathBuf>,
    entries: Vec<Item>,
}

impl OrderVersion {
    /// 这一版是按什么排出来的。
    pub fn mode(&self) -> SortMode {
        self.mode
    }

    /// 这一版有多少项。
    pub fn len(&self) -> usize {
        if self.entries.is_empty() {
            self.paths.len()
        } else {
            self.entries.len()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.paths.is_empty()
    }

    /// 这一版记下的条目。旧存档只有路径，按顶层文件补出来。
    fn items(&self) -> Vec<Item> {
        if self.entries.is_empty() {
            self.paths.iter().cloned().map(Item::file).collect()
        } else {
            self.entries.clone()
        }
    }

    /// 判断「顺序变没变」用的指纹：只看路径和层级，折叠状态与加入时间不算。
    fn key(&self) -> Vec<(PathBuf, u16)> {
        order_key(&self.items())
    }
}

/// 一组条目的排列指纹。
pub(super) fn order_key(items: &[Item]) -> Vec<(PathBuf, u16)> {
    items
        .iter()
        .map(|item| (item.path().to_path_buf(), item.depth()))
        .collect()
}

impl Playlist {
    pub fn sort_mode(&self) -> SortMode {
        self.sort
    }

    /// 历史顺序，最新的一版在前。
    pub fn history(&self) -> &[OrderVersion] {
        &self.history
    }

    /// 按 `mode` 排一次。返回顺序是否真的变了——没变时不留历史，
    /// 免得连点两下「按名称」就把三格历史全占了。
    pub fn sort_items(&mut self, mode: SortMode) -> bool {
        // 手工顺序只能靠拖动得到，没有「一键按手工排」这回事。
        if mode == SortMode::Manual {
            return false;
        }
        let snapshot = self.items.clone();
        let current = self.current().cloned();

        let mut tree = build_tree(std::mem::take(&mut self.items));
        let mut stamps = HashMap::new();
        sort_level(&mut tree, mode, &mut stamps);
        self.items = flatten(tree);

        self.follow(current);
        if order_key(&snapshot) == order_key(&self.items) {
            self.sort = mode;
            return false;
        }
        self.push_history(self.sort, snapshot);
        self.sort = mode;
        true
    }

    /// 恢复到历史里的第 `index` 版。顺序已经一样时返回 `false`。
    ///
    /// 还原本身也是一次改顺序，所以还原前的那一版同样进历史——顺手多一次后悔机会。
    pub fn restore_version(&mut self, index: usize) -> bool {
        let Some(version) = self.history.get(index).cloned() else {
            return false;
        };
        let snapshot = self.items.clone();
        if !self.apply_version(&version) {
            return false;
        }
        self.push_history(self.sort, snapshot);
        self.sort = version.mode;
        true
    }

    /// 记一版旧顺序。同一个排列只留一份，最多留 [`HISTORY_LIMIT`] 份。
    pub(super) fn push_history(&mut self, mode: SortMode, entries: Vec<Item>) {
        let key = order_key(&entries);
        self.history.retain(|version| version.key() != key);
        self.history.insert(
            0,
            OrderVersion {
                mode,
                paths: Vec::new(),
                entries,
            },
        );
        self.history.truncate(HISTORY_LIMIT);
    }

    /// 按旧版本重排。快照里没有的项（之后才加进来的）接在末尾，保持它们之间
    /// 原有的先后。返回顺序是否真的变了。
    fn apply_version(&mut self, version: &OrderVersion) -> bool {
        let before = order_key(&self.items);
        let current = self.current().cloned();
        let mut pool = std::mem::take(&mut self.items);
        let mut ordered = Vec::with_capacity(pool.len());
        for entry in version.items() {
            if let Some(position) = pool.iter().position(|item| item.path == entry.path) {
                let mut item = pool.remove(position);
                // 层级跟着旧版本回来，否则还原出来是一张散架的树。
                item.set_depth(entry.depth());
                ordered.push(item);
            }
        }
        ordered.append(&mut pool);
        self.items = ordered;
        self.follow(current);
        before != order_key(&self.items)
    }
}

// -- 树内排序 --------------------------------------------------------------

/// 排序用的嵌套结构：一层排完再看下一层。
struct Branch {
    item: Item,
    children: Vec<Branch>,
}

/// 把「扁平数组 + 层级」还原成嵌套结构。
///
/// 层级跳跃（手工改坏的存档）按当前层收下，而不是把这一项丢掉——宁可行画在
/// 浅一层，也不能让用户的文件凭空消失。
fn build_tree(items: Vec<Item>) -> Vec<Branch> {
    build_level(&items, 0, 0).0
}

fn build_level(items: &[Item], start: usize, depth: u16) -> (Vec<Branch>, usize) {
    let mut level = Vec::new();
    let mut pos = start;
    while pos < items.len() {
        let mut item = items[pos].clone();
        if item.depth() < depth {
            break;
        }
        item.set_depth(depth);
        pos += 1;
        let mut branch = Branch {
            item,
            children: Vec::new(),
        };
        if branch.item.is_dir() {
            let (children, next) = build_level(items, pos, depth + 1);
            branch.children = children;
            pos = next;
        }
        level.push(branch);
    }
    (level, pos)
}

fn flatten(branches: Vec<Branch>) -> Vec<Item> {
    let mut out = Vec::new();
    push_branches(branches, &mut out);
    out
}

fn push_branches(branches: Vec<Branch>, out: &mut Vec<Item>) {
    for branch in branches {
        out.push(branch.item);
        push_branches(branch.children, out);
    }
}

/// 逐层排序：先把这一层排好，再递归子层——子项永远跟着父项走。
fn sort_level(branches: &mut [Branch], mode: SortMode, stamps: &mut HashMap<PathBuf, Option<i64>>) {
    branches.sort_by(|left, right| compare(mode, left, right, stamps));
    for branch in branches.iter_mut() {
        sort_level(&mut branch.children, mode, stamps);
    }
}

fn compare(
    mode: SortMode,
    left: &Branch,
    right: &Branch,
    stamps: &mut HashMap<PathBuf, Option<i64>>,
) -> std::cmp::Ordering {
    // 文件夹排在文件前面：和资源管理器一致，找剧集时先看到目录。
    let by_kind = right.item.is_dir().cmp(&left.item.is_dir());
    if by_kind != std::cmp::Ordering::Equal {
        return by_kind;
    }
    match mode {
        SortMode::Name => natural_cmp(left.item.path(), right.item.path()),
        SortMode::Added => left.item.added_at().cmp(&right.item.added_at()),
        SortMode::Modified => {
            let left_stamp = modified_rank(left.item.path(), stamps);
            let right_stamp = modified_rank(right.item.path(), stamps);
            match (left_stamp, right_stamp) {
                (Some(left), Some(right)) => left.cmp(&right),
                // 读不到修改时间（文件被移走、权限不足）的排在最后，而不是挤到最前面。
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        }
        // 手工顺序只能靠拖动得到，调用方已经挡掉了。
        SortMode::Manual => std::cmp::Ordering::Equal,
    }
}

/// 取一次修改时间就缓存住：比较函数会被调用很多次，每次都 `metadata` 一遍
/// 在机械盘和网络盘上慢得能看出来。
fn modified_rank(path: &Path, stamps: &mut HashMap<PathBuf, Option<i64>>) -> Option<i64> {
    *stamps
        .entry(path.to_path_buf())
        .or_insert_with(|| modified_secs(path))
}

/// 文件最后修改时间（Unix 秒）。读不到就是 `None`。
fn modified_secs(path: &Path) -> Option<i64> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_secs().min(i64::MAX as u64) as i64)
}
