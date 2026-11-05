//! 树形结构：文件夹的递归展开、折叠与拖放移动。
//!
//! 树用「扁平数组 + 层级」表示：`items[i]` 的缩进是 `depth`，子项紧跟在父项
//! 后面（深度优先）。这样播放顺序天然就是数组顺序，翻页、去重、存档都不用
//! 在两套结构之间同步；折叠只是渲染和「下一集」时跳过一段连续区间。
//!
//! 不变量：子项的层级最多比前一项深一层，而且前一项必须是文件夹；不满足的
//! 数据一律在 [`Playlist::sanitize`] 里压平，免得拖出孤儿行。

use super::scan::natural_cmp;
use super::{Item, Playlist};
use crate::formats;
use std::path::Path;

/// 一次最多展开几层。符号链接已经跳过，这个上限防的是超深目录树。
const MAX_DEPTH: u16 = 16;
/// 一次最多收进来多少项：误拖一个挂着几万个文件的根目录时，界面要还能用。
const MAX_ITEMS: usize = 4000;

/// 拖放的落点。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropSpot {
    /// 插到这一行之前，层级跟着这一行。
    Before(usize),
    /// 插到这一行之后（文件夹则是整棵子树之后）。
    After(usize),
    /// 放进这个文件夹，成为它的第一个/最后一个子项。
    Into(usize),
}

impl DropSpot {
    /// 落点压在哪一行。拖动回调据此确认这条落点还是不是当前行。
    pub fn anchor(self) -> usize {
        match self {
            DropSpot::Before(index) | DropSpot::After(index) | DropSpot::Into(index) => index,
        }
    }

    pub fn is_into(self) -> bool {
        matches!(self, DropSpot::Into(_))
    }
}

/// 把目录递归展开成一棵列表树（文件夹节点 + 里面的媒体文件）。
///
/// 里面一个能播的文件都没有的文件夹不会留下空壳：加了一个空目录之后列表
/// 里多出一行点不动的东西，只会让人以为出错。
pub fn collect_tree(dir: &Path) -> Vec<Item> {
    let mut out = Vec::new();
    let mut budget = MAX_ITEMS;
    push_dir(&mut out, dir, 0, &mut budget);
    out
}

/// 返回这个文件夹是否留下了节点。
fn push_dir(out: &mut Vec<Item>, dir: &Path, depth: u16, budget: &mut usize) -> bool {
    if depth > MAX_DEPTH || *budget == 0 {
        return false;
    }
    let entries = read_level(dir);
    if entries.is_empty() {
        return false;
    }

    let start = out.len();
    out.push(Item::folder(dir.to_path_buf()).with_depth(depth));
    for (path, is_dir) in entries {
        if *budget == 0 {
            break;
        }
        if is_dir {
            push_dir(out, &path, depth + 1, budget);
        } else {
            *budget -= 1;
            out.push(Item::file(path).with_depth(depth + 1));
        }
    }

    if out.len() == start + 1 {
        out.pop();
        return false;
    }
    true
}

/// 一层目录里的条目：(路径, 是不是文件夹)。文件夹在前，各自按自然序。
fn read_level(dir: &Path) -> Vec<(std::path::PathBuf, bool)> {
    let mut entries: Vec<(std::path::PathBuf, bool)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            // 符号链接一律不跟：目录链接可以指回父目录，递归下去就是死循环。
            // 媒体库里用链接组织的目录需要手动单独添加。
            if file_type.is_symlink() {
                return None;
            }
            let path = entry.path();
            if file_type.is_dir() {
                Some((path, true))
            } else if formats::is_media(&path) {
                Some((path, false))
            } else {
                None
            }
        })
        .collect();
    // 文件夹排在文件前面：和资源管理器一致，找剧集时先看到目录。
    entries.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| natural_cmp(&left.0, &right.0))
    });
    entries
}

impl Playlist {
    /// 第 `index` 项的子树的结束位置（不含）。没有子项时就是 `index + 1`。
    pub fn subtree_end(&self, index: usize) -> usize {
        let Some(item) = self.items().get(index) else {
            return index.min(self.items().len());
        };
        let depth = item.depth();
        let mut end = index + 1;
        while end < self.items().len() && self.items()[end].depth() > depth {
            end += 1;
        }
        end
    }

    /// 文件夹里有多少个可播放的文件（含更深层）。
    pub fn dir_file_count(&self, index: usize) -> usize {
        let end = self.subtree_end(index);
        self.items()
            .get(index + 1..end)
            .map(|children| children.iter().filter(|item| !item.is_dir()).count())
            .unwrap_or(0)
    }

    /// `index` 是不是 `ancestor` 的后代。界面据此把「正在播的那一项藏在里面」
    /// 标到文件夹行上。
    pub fn is_descendant(&self, index: usize, ancestor: usize) -> bool {
        index > ancestor && index < self.subtree_end(ancestor)
    }

    /// 列表里有没有文件夹。没有的话「折叠/展开所有」没有任何可做的。
    pub fn has_dir(&self) -> bool {
        self.items().iter().any(|item| item.is_dir())
    }

    /// 是不是所有文件夹都收起来了。没有文件夹时返回 `false`——没有可展开的
    /// 东西，不该让按钮显示成「已经全收起了」。
    pub fn all_collapsed(&self) -> bool {
        let mut seen = false;
        for item in self.items() {
            if item.is_dir() {
                seen = true;
                if !item.collapsed() {
                    return false;
                }
            }
        }
        seen
    }

    /// 一键收起或展开所有文件夹，返回是否真的有变化。
    pub fn set_all_collapsed(&mut self, collapsed: bool) -> bool {
        let mut changed = false;
        for item in self.items_mut() {
            if item.is_dir() && item.collapsed() != collapsed {
                item.set_collapsed(collapsed);
                changed = true;
            }
        }
        changed
    }

    /// 展开或收起一个文件夹。文件夹以外的行返回 `false`。
    pub fn toggle_dir(&mut self, index: usize) -> bool {
        let Some(item) = self.items_mut().get_mut(index) else {
            return false;
        };
        if !item.is_dir() {
            return false;
        }
        item.toggle_collapsed();
        true
    }

    /// 展开 `index` 的所有祖先。返回是否真的展开了什么。
    ///
    /// 拖动、跳转之后调用：刚移进收起文件夹的项会立刻消失在界面上。
    pub fn reveal(&mut self, index: usize) -> bool {
        let Some(depth) = self.items().get(index).map(Item::depth) else {
            return false;
        };
        let mut changed = false;
        let mut level = depth;
        let mut pos = index;
        while pos > 0 {
            pos -= 1;
            let item = &mut self.items_mut()[pos];
            if item.depth() < level {
                if item.collapsed() {
                    item.set_collapsed(false);
                    changed = true;
                }
                level = item.depth();
                if level == 0 {
                    break;
                }
            }
        }
        changed
    }

    /// 界面上要画出来的行。收起文件夹时，它后面的整棵子树一起跳过。
    ///
    /// 一次遍历算完，而不是在渲染每一行时各自往上看祖先：那样是 O(n²)，
    /// 目录大的列表每帧都会卡。
    pub fn visible_indices(&self) -> Vec<usize> {
        let items = self.items();
        let mut rows = Vec::with_capacity(items.len());
        let mut folded: Option<u16> = None;
        for (index, item) in items.iter().enumerate() {
            if let Some(depth) = folded {
                if item.depth() > depth {
                    continue;
                }
                folded = None;
            }
            rows.push(index);
            if item.is_dir() && item.collapsed() {
                folded = Some(item.depth());
            }
        }
        rows
    }

    /// 搜索时要画的行：文件名匹配的文件，以及它们的祖先文件夹。
    ///
    /// 搜索不看折叠状态：藏在收起文件夹里的文件也得找得到。文件夹行本身不参与
    /// 匹配，留着只是为了让结果仍然看得出层级。空查询退回 `visible_indices`。
    pub fn search_indices(&self, query: &str) -> Vec<usize> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return self.visible_indices();
        }

        let items = self.items();
        let mut matched = vec![false; items.len()];
        // 当前路径上的祖先：深度回退时弹出，遇到文件夹时压入。
        let mut ancestors: Vec<usize> = Vec::new();
        for (index, item) in items.iter().enumerate() {
            while ancestors.len() > usize::from(item.depth()) {
                ancestors.pop();
            }
            if item.is_dir() {
                ancestors.push(index);
                continue;
            }
            let name = item
                .path()
                .file_name()
                .map(|value| value.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if name.contains(&needle) {
                matched[index] = true;
                for &ancestor in &ancestors {
                    matched[ancestor] = true;
                }
            }
        }

        matched
            .iter()
            .enumerate()
            .filter_map(|(index, keep)| keep.then_some(index))
            .collect()
    }

    /// 拖放：把第 `from` 项的整棵子树搬到 `spot`，层级跟着落点走。
    ///
    /// 顺序没变、落点在自己子树内部、或者落点不是文件夹却要求「放进去」时
    /// 返回 `false`，也不占一格历史。
    pub fn move_to(&mut self, from: usize, spot: DropSpot) -> bool {
        let len = self.items().len();
        if from >= len {
            return false;
        }
        let source_end = self.subtree_end(from);
        let source_depth = self.items()[from].depth();

        // 落点在自己身上或自己的子树里：一律拒绝。把文件夹拖进自己会算出一段
        // 重叠的区间，drain 之后下标就全错了；而「插到自己前面」本来就等于没动。
        if spot.anchor() >= from && spot.anchor() < source_end {
            return false;
        }

        let (insert_at, depth) = match spot {
            DropSpot::Before(target) => {
                if target >= len {
                    (len, 0)
                } else {
                    (target, self.items()[target].depth())
                }
            }
            DropSpot::After(target) => {
                if target >= len {
                    (len, 0)
                } else {
                    // 展开的文件夹「包着」自己的内容，落在它下半截时插到整棵
                    // 子树之后，看着才像和这个文件夹平级。
                    (self.subtree_end(target), self.items()[target].depth())
                }
            }
            DropSpot::Into(target) => {
                if target >= len || !self.items()[target].is_dir() {
                    return false;
                }
                (self.subtree_end(target), self.items()[target].depth() + 1)
            }
        };

        // 落点落在自己的子树内部：那是把文件夹拖进自己，直接拒绝。
        if insert_at > from && insert_at < source_end {
            return false;
        }
        // 位置没变、层级也没变时什么都不做。层级变了（比如把紧跟文件夹后面的
        // 项拖进这个文件夹）仍要执行：那是在改归属，不是在改顺序。
        if (insert_at == from || insert_at == source_end) && depth == source_depth {
            return false;
        }

        let current = self.current().cloned();
        let mode = self.sort;
        let before = self.items().to_vec();

        // 先把目标文件夹展开：拖进去的东西立刻消失会让人以为拖丢了。
        if let DropSpot::Into(target) = spot {
            self.items_mut()[target].set_collapsed(false);
        }

        let mut block: Vec<Item> = self.items_mut().drain(from..source_end).collect();
        let mut at = insert_at;
        if at > from {
            at -= block.len();
        }
        let shift = i32::from(depth) - i32::from(source_depth);
        for item in &mut block {
            item.set_depth((i32::from(item.depth()) + shift).max(0) as u16);
        }
        self.items_mut().splice(at..at, block);

        self.follow(current);
        self.push_history(mode, before);
        self.sort = super::SortMode::Manual;
        true
    }

    /// 旧的扁平拖放接口：插到第 `insert_at` 项之前。
    ///
    /// 层级跟着目标行走——把它拖到某个子项前面，它就成了那个子项的兄弟。
    pub fn move_item(&mut self, from: usize, insert_at: usize) -> bool {
        self.move_to(from, DropSpot::Before(insert_at))
    }
}
