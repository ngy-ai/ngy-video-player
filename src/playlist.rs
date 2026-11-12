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
//!
//! 文件分工：`item` 是一行（文件 / 文件夹），`list` 是一份列表（条目 + 当前项 +
//! 排序记忆 + 改序历史），`set` 是面板上的多页列表与存档；`scan` / `sort` / `tree`
//! 分别是扫描、排序与树操作。

mod item;
mod list;
mod scan;
mod set;
mod sort;
mod tree;

pub use item::Item;
pub use list::{Playlist, Removal};
pub use scan::{collect_dir, parse_playlist};
pub use set::{DEFAULT_TAB_NAME, PlaylistSet, PlaylistTab};
pub use sort::{OrderVersion, SortMode};
pub use tree::{DropSpot, collect_tree};
// 格式转换扫描文件夹时也按这套自然序排：同一个 `第2集` 不该在两个地方排到不同位置。
pub(crate) use scan::natural_cmp;
