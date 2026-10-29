//! 排序、手工拖动重排与顺序历史。
//!
//! 排序是「点一次执行一次」的动作，不是一直生效的规则：排完之后再拖进来一个文件，
//! 它一律追加到末尾——按名字插到中间会让人找不到刚放进来的那一项。当前用的是哪种
//! 排法只记下来给菜单打勾。
//!
//! 每次改顺序之前，先把当时的排列存一份进历史（最多 [`HISTORY_LIMIT`] 份），
//! 排错了能一键退回去。历史存路径而不是下标：文件删掉之后下标会全部错位。

use super::{Item, Playlist, natural_cmp};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

/// 一版旧顺序：改之前是按什么排的、当时都有哪些文件、怎么排的。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OrderVersion {
    mode: SortMode,
    paths: Vec<PathBuf>,
}

impl OrderVersion {
    /// 这一版是按什么排出来的。
    pub fn mode(&self) -> SortMode {
        self.mode
    }

    /// 这一版有多少项。
    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
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
        let current = self.current().cloned();
        let before = self.order();
        match mode {
            SortMode::Name => self
                .items
                .sort_by(|left, right| natural_cmp(&left.path, &right.path)),
            SortMode::Added => self.items.sort_by_key(|item| item.added_at),
            SortMode::Modified => self.sort_by_modified(),
            SortMode::Manual => unreachable!("上面已经挡掉手工排序"),
        }
        let after = self.order();
        self.restore_current(current);
        if before == after {
            self.sort = mode;
            return false;
        }
        self.push_history(self.sort, before);
        self.sort = mode;
        true
    }

    /// 把第 `from` 项挪到 `insert_at`（插入点按**移动前**的下标算）。
    ///
    /// 插在自己前面和后面都等于没动，这时返回 `false`，也不会占用一格历史。
    pub fn move_item(&mut self, from: usize, insert_at: usize) -> bool {
        if from >= self.items.len() {
            return false;
        }
        let target = insert_at.min(self.items.len());
        if target == from || target == from + 1 {
            return false;
        }
        let current = self.current().cloned();
        let before = self.order();
        let item = self.items.remove(from);
        let target = if target > from { target - 1 } else { target };
        self.items.insert(target, item);
        self.restore_current(current);
        self.push_history(self.sort, before);
        self.sort = SortMode::Manual;
        true
    }

    /// 恢复到历史里的第 `index` 版。顺序已经一样时返回 `false`。
    ///
    /// 还原本身也是一次改顺序，所以还原前的那一版同样进历史——顺手多一次后悔机会。
    pub fn restore_version(&mut self, index: usize) -> bool {
        let Some(version) = self.history.get(index).cloned() else {
            return false;
        };
        let before = self.order();
        if !self.apply_order(&version.paths) {
            return false;
        }
        self.push_history(self.sort, before);
        self.sort = version.mode;
        true
    }

    /// 当前的路径排列，用来判断「顺序到底变没变」。
    fn order(&self) -> Vec<PathBuf> {
        self.items.iter().map(|item| item.path.clone()).collect()
    }

    /// 当前项跟着它自己的文件走，而不是留在原来的下标上。
    fn restore_current(&mut self, path: Option<PathBuf>) {
        self.index = path.and_then(|path| self.items.iter().position(|item| item.path == path));
    }

    /// 记一版旧顺序。同一个排列只留一份，最多留 [`HISTORY_LIMIT`] 份。
    fn push_history(&mut self, mode: SortMode, paths: Vec<PathBuf>) {
        self.history.retain(|version| version.paths != paths);
        self.history.insert(0, OrderVersion { mode, paths });
        self.history.truncate(HISTORY_LIMIT);
    }

    /// 按 `order` 重排。快照里没有的项（之后才加进来的）接在末尾，保持它们之间
    /// 原有的先后。返回顺序是否真的变了。
    fn apply_order(&mut self, order: &[PathBuf]) -> bool {
        let before = self.order();
        let current = self.current().cloned();
        let mut pool = std::mem::take(&mut self.items);
        let mut ordered = Vec::with_capacity(pool.len());
        for path in order {
            if let Some(position) = pool.iter().position(|item| &item.path == path) {
                ordered.push(pool.remove(position));
            }
        }
        ordered.append(&mut pool);
        self.items = ordered;
        self.restore_current(current);
        !before.iter().eq(self.items.iter().map(|item| &item.path))
    }

    /// 先取一遍修改时间再排：`sort_by` 的比较函数会被调用很多次，
    /// 每次都 `metadata` 一遍在机械盘和网络盘上慢得能看出来。
    fn sort_by_modified(&mut self) {
        let mut decorated: Vec<(Item, Option<i64>)> = std::mem::take(&mut self.items)
            .into_iter()
            .map(|item| {
                let stamp = modified_secs(&item.path);
                (item, stamp)
            })
            .collect();
        decorated.sort_by(|left, right| match (left.1, right.1) {
            (Some(left), Some(right)) => left.cmp(&right),
            // 读不到修改时间（文件被移走、权限不足）的排在最后，而不是挤到最前面。
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        self.items = decorated.into_iter().map(|(item, _)| item).collect();
    }
}

/// 文件最后修改时间（Unix 秒）。读不到就是 `None`。
fn modified_secs(path: &std::path::Path) -> Option<i64> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_secs().min(i64::MAX as u64) as i64)
}
