//! 工程文件：`.ngyproj`，JSON 明文，可 diff、可手改排查。
//!
//! 三条硬约束（见 `TODO.md` §0.5）：
//!
//! 1. 顶层 `version` + 所有结构体 `#[serde(default)]`，读盘时逐版升级；
//! 2. 素材路径相对工程文件优先，跨盘符时才写绝对路径；
//! 3. 写盘用临时文件 + rename，并把上一版留成 `.bak`。崩溃时最坏丢掉最后一次保存。
//!
//! 素材找不到时工程照常打开：文件不存在只在 [`Asset::missing`] 上打标记，界面据此
//! 提示重链，而不是让整个工程打不开。

use crate::timeline::{AssetId, AssetState, Id, MediaInfo, MediaStamp, Rational, Sequence, Tick};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::{fs, io};

/// 工程文件扩展名（不带点）。
pub const PROJECT_EXTENSION: &str = "ngyproj";
/// 当前工程格式版本。加字段不算新版本（`serde(default)` 能兜住），只有语义变化才 +1。
pub const PROJECT_VERSION: u32 = 1;

/// 工程素材表里的一项。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Asset {
    pub id: AssetId,
    /// 相对工程文件的路径；素材在别的盘符上时退回绝对路径。
    pub path: PathBuf,
    /// 本次会话解析出来的绝对路径。不写进工程文件。
    #[serde(skip)]
    pub resolved: PathBuf,
    /// 打开工程时原文件已经不在。不写进工程文件，下次打开重新判断。
    #[serde(skip)]
    pub missing: bool,
    /// 探测结果缓存。随工程存盘，打开工程不必重探。
    pub media: Option<MediaInfo>,
    /// 探测时的文件指纹。文件改过时用它判断缓存过期。
    pub stamp: Option<MediaStamp>,
}

impl Asset {
    /// 界面上显示的名字。路径为空时给一个不会显示成空白的兜底。
    pub fn display_name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "素材".to_string())
    }

    /// 此刻能不能直接剪这段素材。
    ///
    /// 没探测过的素材算 `Ready`：没有缓存也就没有「过期」可言，真正要不要探测
    /// 由调用方决定。文件被换过（大小或修改时间不同）才算 `Changed`。
    pub fn state(&self) -> AssetState {
        if self.missing {
            return AssetState::Missing;
        }
        if self.resolved.as_os_str().is_empty() {
            return AssetState::Ready;
        }
        if self.media.is_none() {
            return AssetState::Ready;
        }
        match self.stamp {
            Some(stamp) if MediaStamp::of(&self.resolved) == Some(stamp) => AssetState::Ready,
            _ => AssetState::Changed,
        }
    }
}

/// 界面状态：跟着工程一起存，打开工程后回到上次的位置。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    /// 播放头位置。
    pub playhead: Tick,
    /// 选中的片段。加载时会剔除已经不存在的 id。
    pub selected: Vec<Id>,
}

/// 一个工程 = 一条序列 + 素材表 + 界面状态。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Project {
    pub version: u32,
    pub sequence: Sequence,
    pub assets: Vec<Asset>,
    pub ui: UiState,
    /// 素材 id 的计数器。和序列的 id 空间分开：素材表不属于任何一条序列。
    next_asset_id: u64,
    /// 工程文件所在目录。不写进文件：工程文件拷走之后它自然跟着变。
    #[serde(skip)]
    base_dir: PathBuf,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            version: PROJECT_VERSION,
            sequence: Sequence::new("序列 1", Rational::FPS_30, 1920, 1080),
            assets: Vec::new(),
            ui: UiState::default(),
            next_asset_id: 1,
            base_dir: PathBuf::new(),
        }
    }
}

impl Project {
    pub fn new() -> Self {
        Self::default()
    }

    /// 工程文件所在目录。还没保存过时是空路径。
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    pub fn asset(&self, id: AssetId) -> Option<&Asset> {
        self.assets.iter().find(|asset| asset.id == id)
    }

    /// 素材在当前磁盘上的绝对路径。找不到文件时也返回推算出来的位置，
    /// 方便提示「它原来在哪儿」。
    pub fn asset_path(&self, id: AssetId) -> Option<&Path> {
        self.asset(id)
            .map(|asset| asset.resolved.as_path())
            .filter(|path| !path.as_os_str().is_empty())
    }

    /// 登记素材，返回它的 id。同一个文件重复导入只登记一次。
    pub fn add_asset(&mut self, path: &Path) -> AssetId {
        let absolute = absolute_path(path);
        if let Some(existing) = self
            .assets
            .iter()
            .find(|asset| !asset.resolved.as_os_str().is_empty() && asset.resolved == absolute)
        {
            return existing.id;
        }

        let id = self.alloc_asset_id();
        let stored = self.stored_path(&absolute);
        self.assets.push(Asset {
            id,
            path: stored,
            resolved: absolute,
            missing: !path.is_file(),
            media: None,
            stamp: None,
        });
        id
    }

    /// 读工程。JSON 坏掉、版本过新才返回错误；素材缺失不算错误。
    pub fn open(path: &Path) -> Result<Self, ProjectError> {
        let text = fs::read_to_string(path).map_err(|error| ProjectError::Io {
            path: path.to_path_buf(),
            error,
        })?;
        let mut project = Self::from_json(&text).map_err(|error| error.with_path(path))?;
        project.base_dir = path.parent().unwrap_or(Path::new("")).to_path_buf();

        let base_dir = project.base_dir.clone();
        for asset in &mut project.assets {
            resolve_asset(asset, &base_dir);
        }
        project.repair_next_asset_id();
        Ok(project)
    }

    /// 存工程：先写 `.ngyproj.tmp`，旧文件挪成 `.ngyproj.bak`，最后改名。
    ///
    /// 顺便把素材路径重写成相对新位置的形式——「另存为」到别的目录之后，
    /// 工程里不该还留着旧目录的绝对路径。
    pub fn save(&mut self, path: &Path) -> Result<(), ProjectError> {
        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let mut snapshot = self.clone();
        snapshot.version = PROJECT_VERSION;
        snapshot.base_dir.clear();

        for asset in &mut snapshot.assets {
            let absolute = if asset.resolved.as_os_str().is_empty() {
                absolute_path(&asset.path)
            } else {
                asset.resolved.clone()
            };
            asset.path = relative_to(&absolute, &dir).unwrap_or(absolute);
            asset.resolved.clear();
            asset.missing = false;
        }

        let text = serde_json::to_string_pretty(&snapshot).map_err(ProjectError::Json)?;
        write_atomic(path, &text).map_err(|error| ProjectError::Io {
            path: path.to_path_buf(),
            error,
        })?;

        // 内存里的状态跟着落盘结果更新：保存之后立刻再存一次，不该写回旧路径。
        self.version = PROJECT_VERSION;
        self.base_dir = dir;
        self.assets = snapshot.assets;
        let base_dir = self.base_dir.clone();
        for asset in &mut self.assets {
            resolve_asset(asset, &base_dir);
        }
        Ok(())
    }

    /// 解析工程文本。不碰磁盘，测试与导入用。
    pub fn from_json(text: &str) -> Result<Self, ProjectError> {
        let value: serde_json::Value = serde_json::from_str(text)
            .map_err(|error| ProjectError::Parse { path: None, error })?;
        let value = migrate(value)?;
        let mut project: Self = serde_json::from_value(value)
            .map_err(|error| ProjectError::Parse { path: None, error })?;
        project.version = PROJECT_VERSION;
        project.sanitize();
        Ok(project)
    }

    /// 分配一个素材 id。
    pub fn alloc_asset_id(&mut self) -> AssetId {
        let raw = self.next_asset_id.max(1);
        self.next_asset_id = raw.saturating_add(1);
        AssetId::new(raw)
    }

    /// 手改过的工程可能让素材 id 计数器落后，读盘后推一把。
    pub fn repair_next_asset_id(&mut self) {
        let max = self
            .assets
            .iter()
            .map(|asset| asset.id.raw())
            .max()
            .unwrap_or(0);
        self.next_asset_id = self.next_asset_id.max(max.saturating_add(1)).max(1);
    }

    /// 素材相对工程目录的存放形式；跨盘符时退回绝对路径。
    fn stored_path(&self, absolute: &Path) -> PathBuf {
        relative_to(absolute, &self.base_dir).unwrap_or_else(|| absolute.to_path_buf())
    }

    /// 读盘后的自愈：排序、清掉指向已删片段的转场、修 id 计数器。
    ///
    /// 这里不删任何片段：工程里的素材内容比「模型好看」重要，坏数据留给校验报告。
    fn sanitize(&mut self) {
        for track in &mut self.sequence.tracks {
            track.sort_clips();
            track.sort_transitions();
            let known: HashSet<Id> = track.clips.iter().map(|clip| clip.id).collect();
            track.transitions.retain(|transition| {
                known.contains(&transition.before) && known.contains(&transition.after)
            });
        }
        self.sequence
            .markers
            .sort_by_key(|marker| (marker.at, marker.id));
        self.sequence.repair_next_id();
        self.repair_next_asset_id();

        let sequence = &self.sequence;
        self.ui.selected.retain(|id| sequence.clip(*id).is_some());
    }
}

/// 工程文件的读写错误。文案给出可执行的下一步，而不是只报「失败」。
#[derive(Debug)]
pub enum ProjectError {
    Io {
        path: PathBuf,
        error: io::Error,
    },
    Parse {
        path: Option<PathBuf>,
        error: serde_json::Error,
    },
    Json(serde_json::Error),
    /// 工程由更新版本的播放器创建。
    TooNew {
        version: u32,
    },
    /// 顶层不是 JSON 对象。
    NotAProject,
}

impl ProjectError {
    fn with_path(self, path: &Path) -> Self {
        match self {
            ProjectError::Parse { error, .. } => ProjectError::Parse {
                path: Some(path.to_path_buf()),
                error,
            },
            other => other,
        }
    }
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProjectError::Io { path, error } => {
                write!(f, "读写工程文件失败：{}：{error}", path.display())
            }
            ProjectError::Parse { path, error } => match path {
                Some(path) => write!(
                    f,
                    "工程文件不是合法的 JSON：{}：{error}。可以尝试打开同名的 .bak",
                    path.display()
                ),
                None => write!(f, "工程文件不是合法的 JSON：{error}"),
            },
            ProjectError::Json(error) => write!(f, "序列化成工程文件失败：{error}"),
            ProjectError::TooNew { version } => write!(
                f,
                "工程由更新版本的播放器创建（v{version}），当前只支持到 v{PROJECT_VERSION}。请升级后再打开"
            ),
            ProjectError::NotAProject => write!(f, "这不是工程文件：顶层必须是一个 JSON 对象"),
        }
    }
}

impl std::error::Error for ProjectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ProjectError::Io { error, .. } => Some(error),
            ProjectError::Parse { error, .. } | ProjectError::Json(error) => Some(error),
            ProjectError::TooNew { .. } | ProjectError::NotAProject => None,
        }
    }
}

/// 版本迁移：低版本工程逐版升级，高版本直接拒绝（而不是读出一半字段）。
fn migrate(mut value: serde_json::Value) -> Result<serde_json::Value, ProjectError> {
    if !value.is_object() {
        return Err(ProjectError::NotAProject);
    }
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(PROJECT_VERSION as u64) as u32;
    if version > PROJECT_VERSION {
        return Err(ProjectError::TooNew { version });
    }

    // v1 是第一个版本，没有需要搬的字段。以后加版本时在这里 match version 逐级升级，
    // 升到当前版本再反序列化；`serde(default)` 负责补新增字段。
    value["version"] = serde_json::Value::from(PROJECT_VERSION);
    Ok(value)
}

fn resolve_asset(asset: &mut Asset, base_dir: &Path) {
    let candidate = if asset.path.is_absolute() || base_dir.as_os_str().is_empty() {
        asset.path.clone()
    } else {
        base_dir.join(&asset.path)
    };
    asset.missing = !candidate.is_file();
    asset.resolved = absolute_path(&candidate);
}

/// 绝对路径的规范化：只清理 `.` / `..` 与分隔符，不要求文件存在。
fn absolute_path(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// 相对路径：只在同一棵树里才算得出来，跨盘符返回 `None`。
fn relative_to(path: &Path, base: &Path) -> Option<PathBuf> {
    if base.as_os_str().is_empty() {
        return None;
    }
    let path = std::path::absolute(path).ok()?;
    let base = std::path::absolute(base).ok()?;
    path.strip_prefix(base).ok().map(Path::to_path_buf)
}

/// 原子写：临时文件落盘后再改名，旧文件先挪成 `.bak`。
fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }

    let mut temp_name = path.as_os_str().to_os_string();
    temp_name.push(".tmp");
    let temp = PathBuf::from(temp_name);
    fs::write(&temp, text)?;

    let backup = backup_path(path);
    if path.exists() {
        // Windows 上 rename 不能覆盖已有文件，先删掉上一份备份。
        let _ = fs::remove_file(&backup);
        fs::rename(path, &backup)?;
    }
    fs::rename(&temp, path)
}

/// `a.ngyproj` 的备份叫 `a.ngyproj.bak`，和原文件并排存放。
fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".bak");
    PathBuf::from(name)
}

#[cfg(test)]
mod tests;
