//! 一份列表本体：条目数组 + 当前项 + 排序记忆 + 改序历史。

use super::item::Item;
use super::scan::parse_playlist;
use super::sort::{self, OrderVersion, SortMode};
use super::tree::collect_tree;
use crate::formats;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Playlist {
    pub(super) items: Vec<Item>,
    pub(super) index: Option<usize>,
    /// 最后一次是怎么排的。只用来给菜单打勾：新拖进来的文件一律追加到末尾，
    /// 不会因为「按名称排过」就自动插到中间去。
    pub(super) sort: SortMode,
    /// 最近几次改顺序之前的排列，最新的在前，最多 3 份。
    pub(super) history: Vec<OrderVersion>,
}

/// 删除若干项之后的结果。界面据此决定是换歌、停播，还是什么都不用做。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Removal {
    /// 实际删掉的项数（含被一起带走的子项）：越界索引和重复索引不计。
    pub removed: usize,
    /// 其中有多少个是文件。提示语按这个说，免得把文件夹也算成「个文件」。
    pub removed_files: usize,
    /// 被删掉的项里包含当时正在播的那一项。
    pub current_removed: bool,
    /// 删除后应当看作当前项的索引；`None` 表示列表已经空了。
    pub current: Option<usize>,
}

impl Playlist {
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub(super) fn items_mut(&mut self) -> &mut Vec<Item> {
        &mut self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 全部行数，文件夹行也算：标题栏显示的是「列表有多长」。
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// 可播放的文件数，不含文件夹行。
    pub fn file_count(&self) -> usize {
        self.items.iter().filter(|item| !item.dir).count()
    }

    /// 全部可播放文件，按列表顺序。
    ///
    /// 文件夹行只是界面上的分组，这里连同收起文件夹里的项一起给出：整页内容才是
    /// 「导入到时间线」和「拼接」要的东西，界面上少画几行不该少导入几个文件。
    pub fn file_paths(&self) -> Vec<PathBuf> {
        self.items
            .iter()
            .filter(|item| !item.dir)
            .map(|item| item.path.clone())
            .collect()
    }

    pub fn index(&self) -> Option<usize> {
        self.index
    }

    pub fn current(&self) -> Option<&PathBuf> {
        self.index.and_then(|index| self.playable_path(index))
    }

    pub fn get(&self, index: usize) -> Option<&PathBuf> {
        self.items.get(index).map(|item| &item.path)
    }

    /// 第 `index` 行指向的可播放文件；文件夹行返回 `None`。
    pub fn playable_path(&self, index: usize) -> Option<&PathBuf> {
        self.items
            .get(index)
            .filter(|item| !item.dir)
            .map(|item| &item.path)
    }

    /// 第一项能播的东西。空列表、或者只加了空文件夹时是 `None`。
    pub fn first_playable(&self) -> Option<usize> {
        self.playable_indices().next()
    }

    /// 整批替换，并把 `start` 设为当前项。
    ///
    /// 换的是一整份新队列，旧队列的排序方式和历史顺序都不再适用，一起丢掉。
    pub fn replace(&mut self, items: Vec<PathBuf>, start: usize) {
        self.items = items.into_iter().map(Item::file).collect();
        self.sort = SortMode::Added;
        self.history.clear();
        self.index = (!self.items.is_empty())
            .then(|| start.min(self.items.len() - 1))
            .filter(|index| !self.items[*index].dir);
    }

    /// 整批替换成一棵已经展开好的树，并把焦点放到 `focus` 上。
    ///
    /// 「打开文件夹」走这条路：列表里留下文件夹本身和它的内容，而不是一堆
    /// 看不出出处的平铺文件。
    pub fn replace_entries(&mut self, entries: Vec<Item>, focus: Option<&Path>) {
        self.items = entries;
        self.sort = SortMode::Added;
        self.history.clear();
        self.index = focus
            .and_then(|target| {
                self.items
                    .iter()
                    .position(|item| !item.dir && item.path == target)
            })
            .or_else(|| self.first_playable());
    }

    pub fn set_index(&mut self, index: usize) {
        if self.items.get(index).is_some_and(|item| !item.dir) {
            self.index = Some(index);
        }
    }

    /// 把一批已经展开好的条目并进列表，返回新增的可播放文件数。
    ///
    /// 「打开文件」走这条路：目录展开、m3u 解析都在调用方做完，这里只管并进去
    /// 并去重（规则和 [`Playlist::append_paths`] 一致）。
    pub fn append_entries(&mut self, entries: Vec<Item>) -> usize {
        self.push_entries(entries)
    }

    /// 追加文件；目录会连同子文件夹一起展开成子树。返回新增的可播放文件数。
    ///
    /// 已经在列表里的项跳过：同一个文件从目录展开、m3u 和手动挑选三条路进来
    /// 都可能重复，去重一次比让用户在列表里找重复项省事。
    pub fn append_paths(&mut self, paths: &[PathBuf]) -> usize {
        let mut added = 0;
        for path in paths {
            if path.is_dir() {
                added += self.push_entries(collect_tree(path));
            } else if formats::is_media(path) {
                added += usize::from(self.push_file(path.clone()));
            } else if formats::is_playlist(path) {
                for entry in parse_playlist(path) {
                    added += usize::from(self.push_file(entry));
                }
            }
        }
        added
    }

    fn push_file(&mut self, path: PathBuf) -> bool {
        if self.items.iter().any(|item| item.path == path) {
            return false;
        }
        self.items.push(Item::file(path));
        true
    }

    /// 把一棵展开好的子树并进列表：已经在列表里的项连同它的子树一起跳过。
    fn push_entries(&mut self, entries: Vec<Item>) -> usize {
        let mut known: HashSet<PathBuf> = self.items.iter().map(|item| item.path.clone()).collect();
        // 一个新文件都没有时整棵跳过：把一个空壳文件夹塞进列表，用户点开
        // 也只会看到一堆「已经有了」的东西。
        if !entries
            .iter()
            .any(|item| !item.dir && !known.contains(&item.path))
        {
            return 0;
        }
        let mut added = 0;
        let mut skip_depth: Option<u16> = None;
        for item in entries {
            if let Some(depth) = skip_depth {
                if item.depth > depth {
                    continue;
                }
                skip_depth = None;
            }
            if known.contains(&item.path) {
                if item.dir {
                    skip_depth = Some(item.depth);
                }
                continue;
            }
            if !item.dir {
                added += 1;
            }
            known.insert(item.path.clone());
            self.items.push(item);
        }
        added
    }

    /// 下一项。文件夹和收起的子树都不参与播放——收起就是「这段先不看了」。
    pub fn next_index(&self, wrap: bool) -> Option<usize> {
        let current = self.index?;
        let playable: Vec<usize> = self.playable_indices().collect();
        if let Some(next) = playable.iter().copied().find(|index| *index > current) {
            return Some(next);
        }
        wrap.then(|| playable.first().copied()).flatten()
    }

    pub fn previous_index(&self, wrap: bool) -> Option<usize> {
        let current = self.index?;
        let playable: Vec<usize> = self.playable_indices().collect();
        if let Some(previous) = playable
            .iter()
            .rev()
            .copied()
            .find(|index| *index < current)
        {
            return Some(previous);
        }
        wrap.then(|| playable.last().copied()).flatten()
    }

    /// 参与播放的行：是文件，而且没有藏在收起的文件夹里。
    fn playable_indices(&self) -> impl Iterator<Item = usize> + '_ {
        let items = &self.items;
        self.visible_indices()
            .into_iter()
            .filter(move |index| !items[*index].dir)
    }

    /// 删除一项，等价于 `remove_many(&[index])`。
    pub fn remove(&mut self, index: usize) -> Removal {
        self.remove_many(&[index])
    }

    /// 批量删除。`indexes` 用的是**删除前**的索引；删文件夹会连同里面的项一起删。
    ///
    /// 当前项被删掉时不是简单地停住：它落到后面第一个幸存者，后面没有了就退回
    /// 前面最后一个——和用户点「下一集」的直觉一致。列表被删空时不留当前项。
    pub fn remove_many(&mut self, indexes: &[usize]) -> Removal {
        // 目录要连带子树一起删：只删掉那一行的话，子项会变成没有父项的孤儿行。
        let mut targets: Vec<usize> = Vec::new();
        for index in indexes {
            if *index < self.items.len() {
                targets.extend(*index..self.subtree_end(*index));
            }
        }
        targets.sort_unstable();
        targets.dedup();
        if targets.is_empty() {
            return Removal {
                removed: 0,
                removed_files: 0,
                current_removed: false,
                current: self.index,
            };
        }

        let removed_files = targets
            .iter()
            .filter(|index| !self.items[**index].dir)
            .count();
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
        .filter(|index| self.items.get(*index).is_some_and(|item| !item.dir));

        Removal {
            removed: targets.len(),
            removed_files,
            current_removed,
            current: self.index,
        }
    }

    /// 清空列表。只丢列表本身，磁盘上的文件一动不动。
    pub fn clear(&mut self) {
        self.items.clear();
        self.index = None;
        self.sort = SortMode::Added;
        self.history.clear();
    }

    /// 当前项跟着它自己的文件走，而不是留在原来的下标上。
    pub(crate) fn follow(&mut self, path: Option<PathBuf>) {
        self.index = path.and_then(|path| {
            self.items
                .iter()
                .position(|item| !item.dir && item.path == path)
        });
    }

    /// 从存档读回来之后校正：索引可能指向已经不存在的项，层级可能被手改成
    /// 跳过一整层，历史顺序也可能超过上限份。
    pub(super) fn sanitize(&mut self) {
        self.index = self
            .index
            .filter(|index| self.items.get(*index).is_some_and(|item| !item.dir));
        // 子项最多比前一项深一层，而且前一项必须是文件夹。跳跃的层级会让
        // 「子树到哪儿结束」算错，进而把行画到别人的缩进里。
        let mut max_depth = 0u16;
        for item in &mut self.items {
            if item.depth > max_depth {
                item.depth = max_depth;
            }
            max_depth = if item.dir { item.depth + 1 } else { item.depth };
        }
        self.history.retain(|version| !version.is_empty());
        self.history.truncate(sort::HISTORY_LIMIT);
    }
}
