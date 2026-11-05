//! 播放列表。
//!
//! 刻意不把列表交给 mpv 的 `playlist-*` 命令：那套状态在切歌、拖放、手动重排时
//! 会和界面各记一份，两边一旦不同步就很难查。这里 UI 是唯一事实来源，mpv 每次只
//! 负责「放这一个文件」。
//!
//! 列表是**一棵树**：添加文件夹时连同子文件夹一起收进来，文件夹行可以折叠，
//! 也可以把别的行拖进去。树用「扁平数组 + 层级」表示（见 [`tree`]），播放顺序
//! 就是数组顺序，所以翻页、去重、存档不需要在两套结构之间同步。
//!
//! 面板上可以有多个列表（一页一个），整组列表连同各自的位置一起存盘，
//! 下次启动原样恢复——「记住上次在听哪张专辑」比记住窗口大小更贴近使用习惯。

use crate::formats;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

mod scan;
mod sort;
mod tree;

pub use scan::{collect_dir, parse_playlist};
pub use sort::{OrderVersion, SortMode};
pub use tree::{DropSpot, collect_tree};
// 格式转换扫描文件夹时也按这套自然序排：同一个 `第2集` 不该在两个地方排到不同位置。
pub(crate) use scan::natural_cmp;

/// 列表里的一项：一个文件，或者一个文件夹。
///
/// 除了路径还记着「什么时候加进来的」：按添加时间排序靠它。老存档里只有裸路径，
/// 读回来记 0——这样按添加时间排出来的正好还是存档里原来的顺序。
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Item {
    path: PathBuf,
    added_at: i64,
    /// 缩进层级：0 是顶层，文件夹里的项是 1，更深层依次加一。
    depth: u16,
    /// 文件夹节点：自己不能播，只用来折叠和接住拖进来的项。
    dir: bool,
    /// 文件夹是否收起。收起后子项不渲染，也不参与「下一集」。
    collapsed: bool,
}

impl Item {
    /// 一个可播放的文件，放在列表顶层。
    pub fn file(path: PathBuf) -> Self {
        Self {
            path,
            added_at: now_secs(),
            depth: 0,
            dir: false,
            collapsed: false,
        }
    }

    /// 一个文件夹节点。
    pub(super) fn folder(path: PathBuf) -> Self {
        Self {
            path,
            added_at: now_secs(),
            depth: 0,
            dir: true,
            collapsed: false,
        }
    }

    pub(super) fn with_depth(mut self, depth: u16) -> Self {
        self.depth = depth;
        self
    }

    pub(super) fn set_depth(&mut self, depth: u16) {
        self.depth = depth;
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 加入列表的时刻（Unix 秒）；从老存档读回来的项是 0。
    pub fn added_at(&self) -> i64 {
        self.added_at
    }

    pub fn depth(&self) -> u16 {
        self.depth
    }

    pub fn is_dir(&self) -> bool {
        self.dir
    }

    pub fn collapsed(&self) -> bool {
        self.collapsed
    }

    pub(super) fn toggle_collapsed(&mut self) {
        self.collapsed = !self.collapsed;
    }

    pub(super) fn set_collapsed(&mut self, collapsed: bool) {
        self.collapsed = collapsed;
    }
}

/// 旧存档的列表项是一个裸路径字符串，新存档是对象；两种都认。
///
/// 认不出老格式的代价是整份播放列表退回默认值，用户丢的是攒了很久的队列，
/// 所以这里必须兼容。层级/文件夹/折叠三个新字段缺失时都取默认值，老存档
/// 读出来就是一张没有文件夹的平铺列表，行为和以前完全一样。
impl<'de> Deserialize<'de> for Item {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Path(PathBuf),
            Full {
                path: PathBuf,
                #[serde(default)]
                added_at: i64,
                #[serde(default)]
                depth: u16,
                #[serde(default)]
                dir: bool,
                #[serde(default)]
                collapsed: bool,
            },
        }

        Ok(match Raw::deserialize(deserializer)? {
            Raw::Path(path) => Self {
                path,
                added_at: 0,
                depth: 0,
                dir: false,
                collapsed: false,
            },
            Raw::Full {
                path,
                added_at,
                depth,
                dir,
                collapsed,
            } => Self {
                path,
                added_at,
                depth,
                dir,
                collapsed,
            },
        })
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Playlist {
    items: Vec<Item>,
    index: Option<usize>,
    /// 最后一次是怎么排的。只用来给菜单打勾：新拖进来的文件一律追加到末尾，
    /// 不会因为「按名称排过」就自动插到中间去。
    sort: SortMode,
    /// 最近几次改顺序之前的排列，最新的在前，最多 3 份。
    history: Vec<OrderVersion>,
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
    pub(super) fn follow(&mut self, path: Option<PathBuf>) {
        self.index = path.and_then(|path| {
            self.items
                .iter()
                .position(|item| !item.dir && item.path == path)
        });
    }

    /// 从存档读回来之后校正：索引可能指向已经不存在的项，层级可能被手改成
    /// 跳过一整层，历史顺序也可能超过上限份。
    fn sanitize(&mut self) {
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

/// 默认列表的名字。
///
/// 第一页永远是它：名字由程序掌管（存档里改过也不作数），也不能改名、不能删。
/// 「打开文件」和双击文件启动都把内容放进这一页，用户无论怎么整理页签都有个固定落点。
pub const DEFAULT_TAB_NAME: &str = "默认";

/// 面板上的一页：一个名字 + 一份列表。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistTab {
    pub name: String,
    pub playlist: Playlist,
}

impl PlaylistTab {
    fn default_tab() -> Self {
        Self {
            name: DEFAULT_TAB_NAME.to_string(),
            playlist: Playlist::default(),
        }
    }
}

/// 全部列表 + 当前显示的是哪一页。
///
/// 不变量：至少有一页，且第 0 页固定是「默认」列表。它不能被删、不能被改名，
/// 保证界面永远有个落脚点，也保证「打开文件」永远有地方放。
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
    sort: SortMode::Added,
    history: Vec::new(),
};

impl Default for PlaylistSet {
    fn default() -> Self {
        Self {
            tabs: vec![PlaylistTab::default_tab()],
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
            self.tabs.push(PlaylistTab::default_tab());
        }
        let index = self.active();
        &mut self.tabs[index].playlist
    }

    /// 默认列表的下标。它固定是第一页，写成方法是为了让调用方说清楚意图，
    /// 而不是到处散落一个 `0`。
    pub fn default_index(&self) -> usize {
        0
    }

    pub fn is_default(&self, index: usize) -> bool {
        index == self.default_index()
    }

    /// 给某一页改名。页签和「全部列表」菜单都按这个名字显示。
    ///
    /// 返回是否真的改了。默认列表的名字由程序掌管，改不了；别的页也不许叫
    /// 「默认」——两页同名的话，用户分不清哪一页才是「打开」的落点。
    pub fn rename(&mut self, index: usize, name: impl Into<String>) -> bool {
        let name = name.into();
        if self.is_default(index) || name.trim() == DEFAULT_TAB_NAME {
            return false;
        }
        match self.tabs.get_mut(index) {
            Some(tab) => {
                tab.name = name;
                true
            }
            None => false,
        }
    }

    /// 给当前显示的那一页改名。
    pub fn rename_active(&mut self, name: impl Into<String>) -> bool {
        let index = self.active();
        self.rename(index, name)
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

    /// 关掉一页。只剩一页、或者要关的是默认列表时拒绝，返回是否真的关了。
    pub fn remove(&mut self, index: usize) -> bool {
        if self.tabs.len() <= 1 || index >= self.tabs.len() || self.is_default(index) {
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

    /// 按名字找一页（默认页除外）。
    ///
    /// 「打开文件夹」用文件夹名当页名，同名页已经开着就复用它，而不是再堆一页。
    pub fn find_tab(&self, name: &str) -> Option<usize> {
        self.tabs
            .iter()
            .enumerate()
            .find(|(index, tab)| !self.is_default(*index) && tab.name == name)
            .map(|(index, _)| index)
    }

    /// 给新页起一个没被占用的名字：`base`、`base (2)`、`base (3)`……
    ///
    /// 「打开文件夹」直接拿文件夹名当页名。同名文件夹不止一个、或者文件夹正好
    /// 叫「默认」时都得让开：页签重名分不清，而「默认」是保留名。
    pub fn unique_name(&self, base: &str) -> String {
        let base = base.trim();
        let base = if base.is_empty() {
            DEFAULT_TAB_NAME
        } else {
            base
        };
        let mut name = base.to_string();
        let mut number = 2;
        while self.tabs.iter().any(|tab| tab.name == name) {
            name = format!("{base} ({number})");
            number += 1;
        }
        name
    }

    /// 没用过的名字 `列表 N`，跳过已经被占用的号。
    ///
    /// 从 1 数起而不是从页数 +1 数起：第一页固定叫「默认」，按页数算的话
    /// 用户新建的第一页就叫「列表 2」，看着像少了一页。
    pub fn auto_name(&self) -> String {
        let mut number = 1;
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
            self.tabs.push(PlaylistTab::default_tab());
        }
        // 第一页永远是默认列表：老存档里用户自己起的名字在这一版让位，
        // 否则升级之后就没有那个「不能改、不能删、放打开结果」的固定页了。
        self.tabs[0].name = DEFAULT_TAB_NAME.to_string();
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            if tab.name.trim().is_empty() {
                tab.name = format!("列表 {}", index + 1);
            }
            tab.playlist.sanitize();
        }
        self.active = self.active.min(self.tabs.len() - 1);
    }
}
