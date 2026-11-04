//! 工程打包：把素材收进工程目录，让整个工程能整包搬走。
//!
//! 只做「算账」这一半：算出该复制哪些文件、复制完之后工程该长什么样。真正的复制
//! 由界面层扔到后台线程去做——几个 G 的素材同步拷会把界面卡死。
//!
//! 目标目录的结构是平的：所有素材都放进同一个文件夹，重名的加 `-2` 后缀。不做
//! 「按原目录树保留层级」，因为素材常见于不同盘符、不同层级，保留树反而会出现
//! 一堆只有一层空壳的目录。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::Project;

/// 一次打包要做的事。
#[derive(Debug)]
pub struct PackPlan {
    /// 源文件 → 目标文件。界面层按这个顺序复制。
    pub copies: Vec<(PathBuf, PathBuf)>,
    /// 素材路径已经指到包内的工程副本。
    pub project: Project,
}

impl PackPlan {
    /// 这次要复制多少字节，用来判断值不值得提醒用户慢慢等。
    pub fn total_bytes(&self) -> u64 {
        self.copies
            .iter()
            .filter_map(|(from, _)| std::fs::metadata(from).ok())
            .map(|meta| meta.len())
            .sum()
    }
}

#[derive(Debug)]
pub enum PackError {
    /// 工程里没有素材，没什么可收集的。
    Empty,
    /// 有素材已经不在原处：先找回它，否则包里会缺文件。
    Missing(String),
}

impl std::fmt::Display for PackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackError::Empty => write!(f, "工程里还没有素材，没什么可收集的"),
            PackError::Missing(name) => {
                write!(f, "素材「{name}」已经不在原来的位置，先重新链接它再收集")
            }
        }
    }
}

/// 算出打包计划。`root` 是素材要放进去的目录，调用方负责创建。
pub fn plan(project: &Project, root: &Path) -> Result<PackPlan, PackError> {
    if project.assets.is_empty() {
        return Err(PackError::Empty);
    }

    let mut used: HashSet<String> = HashSet::new();
    let mut copies = Vec::new();
    let mut packed = project.clone();

    for asset in &mut packed.assets {
        if asset.resolved.as_os_str().is_empty() {
            continue;
        }
        if !asset.resolved.exists() {
            return Err(PackError::Missing(asset.display_name()));
        }

        let name = asset
            .resolved
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "clip".to_string());
        // 已经在包里了（重复执行收集）：不复制，只把路径钉住。
        if asset.resolved.parent() == Some(root) {
            used.insert(name);
            continue;
        }

        let target = root.join(unique_name(&name, &mut used));
        copies.push((asset.resolved.clone(), target.clone()));
        // `save` 会把路径重算成相对工程文件的相对路径，这里只保证 `resolved` 指向包内。
        asset.resolved = target;
        asset.missing = false;
    }

    Ok(PackPlan {
        copies,
        project: packed,
    })
}

/// 同一批素材里的重名文件：`x.mp4` 已经有主了就换成 `x-2.mp4`。
fn unique_name(name: &str, used: &mut HashSet<String>) -> String {
    if used.insert(name.to_string()) {
        return name.to_string();
    }
    let path = Path::new(name);
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "clip".to_string());
    let extension = path
        .extension()
        .map(|ext| ext.to_string_lossy().into_owned());
    let mut index = 2;
    loop {
        let candidate = match &extension {
            Some(extension) => format!("{stem}-{index}.{extension}"),
            None => format!("{stem}-{index}"),
        };
        if used.insert(candidate.clone()) {
            return candidate;
        }
        index += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ngy-pack-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(path, b"x").expect("写临时文件");
    }

    #[test]
    fn 同名素材落成不同文件() {
        let dir = scratch("dup");
        let first = dir.join("a").join("clip.mp4");
        let second = dir.join("b").join("clip.mp4");
        touch(&first);
        touch(&second);

        let mut project = Project::new();
        project.add_asset(&first);
        project.add_asset(&second);

        let root = dir.join("packed");
        let plan = plan(&project, &root).expect("能打包");
        assert_eq!(plan.copies.len(), 2);
        let names: Vec<_> = plan
            .copies
            .iter()
            .map(|(_, to)| to.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["clip.mp4", "clip-2.mp4"], "第二个重名的加后缀");
        assert!(
            plan.copies
                .iter()
                .all(|(_, to)| to.parent() == Some(root.as_path()))
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 缺失素材直接报错不让打包() {
        let dir = scratch("missing");
        let path = dir.join("gone.mp4");
        let mut project = Project::new();
        project.add_asset(&path);

        let error = plan(&project, &dir.join("packed")).expect_err("缺文件不能打包");
        assert!(matches!(error, PackError::Missing(_)));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 空工程没什么可收集() {
        let dir = scratch("empty");
        let project = Project::new();
        assert!(matches!(
            plan(&project, &dir.join("packed")),
            Err(PackError::Empty)
        ));
        let _ = fs::remove_dir_all(&dir);
    }
}
