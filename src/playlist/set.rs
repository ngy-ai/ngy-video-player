//! 面板上的多页列表：一页一个名字 + 一份列表，整组存档。

use super::list::Playlist;
use super::sort::SortMode;
use serde::{Deserialize, Serialize};

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
