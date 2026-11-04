//! 自动保存与崩溃恢复。
//!
//! 规矩只有三条：
//!
//! 1. 编辑后 **30 秒没有新操作**才落一次 `.autosave`——每改一次都重新计时；
//! 2. 正常保存或正常退出时删掉它，不给下次启动留噪音；
//! 3. 启动时只有比工程文件**更晚**的 `.autosave` 才提示恢复；用户拒绝之后把它
//!    改名成 `.autosave.rejected`，同一份不再重复弹窗，但内容留着可以手动打开。
//!
//! 不做版本历史、不做多份快照轮转（见 `TODO.md` M1-T06 的「不做」）。
//!
//! 计时用调用方传进来的 [`Duration`]（程序启动以来的时长）而不是 `Instant::now()`，
//! 这样状态机可以在单测里直接快进 30 秒，不需要真的睡。

use crate::timeline::{Project, ProjectError};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// 编辑后多久没有新操作就落一次自动保存。
pub const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(30);

/// 自动保存的文件后缀，接在工程文件名后面：`a.ngyproj` → `a.ngyproj.autosave`。
const AUTOSAVE_SUFFIX: &str = ".autosave";
/// 用户拒绝恢复后的后缀。文件保留，但不再参与「待恢复」判断。
const REJECTED_SUFFIX: &str = ".autosave.rejected";

/// 自动保存的计时器。只回答「现在该不该写」，读写文件在下面的函数里。
#[derive(Clone, Debug)]
pub struct AutoSave {
    dirty_at: Option<Duration>,
    interval: Duration,
}

impl Default for AutoSave {
    fn default() -> Self {
        Self::new()
    }
}

impl AutoSave {
    pub fn new() -> Self {
        Self::with_interval(AUTOSAVE_INTERVAL)
    }

    /// 自定义间隔，测试用来快进。
    pub fn with_interval(interval: Duration) -> Self {
        Self {
            dirty_at: None,
            interval,
        }
    }

    /// 记一次编辑。每改一次都重新计时：要的是「30 秒没有新操作」，不是「30 秒必写」。
    pub fn mark_dirty(&mut self, now: Duration) {
        self.dirty_at = Some(now);
    }

    /// 正常保存之后不再需要自动保存。
    pub fn mark_saved(&mut self) {
        self.dirty_at = None;
    }

    /// 是否还有没落盘的改动。
    pub fn is_dirty(&self) -> bool {
        self.dirty_at.is_some()
    }

    /// 到点了吗？到点就清掉计时，调用方接着写文件。
    ///
    /// 写入失败时调用方可以再 `mark_dirty(now)` 把它重新排上。
    pub fn should_write(&mut self, now: Duration) -> bool {
        match self.dirty_at {
            Some(at) if now.saturating_sub(at) >= self.interval => {
                self.dirty_at = None;
                true
            }
            _ => false,
        }
    }
}

/// 工程文件对应的自动保存路径。
pub fn autosave_path(project_path: &Path) -> PathBuf {
    with_suffix(project_path, AUTOSAVE_SUFFIX)
}

/// 用户拒绝恢复后自动保存被挪到哪儿。
pub fn rejected_path(project_path: &Path) -> PathBuf {
    with_suffix(project_path, REJECTED_SUFFIX)
}

/// 写一份自动保存。返回落盘路径；失败时工程本身不受影响。
pub fn write(project: &Project, project_path: &Path) -> Result<PathBuf, ProjectError> {
    let path = autosave_path(project_path);
    let text = project.to_json_at(&path)?;
    crate::timeline::project::write_plain(&path, &text).map_err(|error| ProjectError::Io {
        path: path.clone(),
        error,
    })?;
    Ok(path)
}

/// 启动时该不该提示恢复：只有比工程文件更晚的 `.autosave` 才算。
///
/// 工程文件不存在（例如先崩在「新建工程」里）时，任何自动保存都算数。
pub fn pending(project_path: &Path) -> Option<PathBuf> {
    let autosave = autosave_path(project_path);
    let saved_at = modified(&autosave)?;
    match modified(project_path) {
        Some(project_at) if saved_at <= project_at => None,
        _ => Some(autosave),
    }
}

/// 用户拒绝恢复：改名而不是删除，内容留着以后手动打开。同一份不再提示。
pub fn dismiss(project_path: &Path) -> io::Result<PathBuf> {
    let from = autosave_path(project_path);
    let to = rejected_path(project_path);
    // Windows 上 rename 不能覆盖已有文件：旧的那份拒绝记录先删掉。
    let _ = fs::remove_file(&to);
    fs::rename(&from, &to)?;
    Ok(to)
}

/// 正常保存或正常退出时删掉自动保存。删不掉也不该阻止退出。
pub fn clear(project_path: &Path) {
    let _ = fs::remove_file(autosave_path(project_path));
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests;
