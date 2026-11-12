//! 列表里的一项：文件或文件夹，以及它「什么时候加进来的」「缩进几层」。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 列表里的一项：一个文件，或者一个文件夹。
///
/// 除了路径还记着「什么时候加进来的」：按添加时间排序靠它。老存档里只有裸路径，
/// 读回来记 0——这样按添加时间排出来的正好还是存档里原来的顺序。
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Item {
    pub(super) path: PathBuf,
    pub(super) added_at: i64,
    /// 缩进层级：0 是顶层，文件夹里的项是 1，更深层依次加一。
    pub(super) depth: u16,
    /// 文件夹节点：自己不能播，只用来折叠和接住拖进来的项。
    pub(super) dir: bool,
    /// 文件夹是否收起。收起后子项不渲染，也不参与「下一集」。
    pub(super) collapsed: bool,
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
