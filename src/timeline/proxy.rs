//! 代理（低码率预览副本）的选用规则。
//!
//! 规则只有两条，都做成纯函数，好让「什么时候用代理」「导出为什么拒绝」能被单测钉住：
//!
//! 1. 素材比序列大（长边更长）且同名代理文件在，预览就用代理；否则用原片。
//! 2. 代理只给预览用，**导出必须用原片**——素材路径本身就是 `*.proxy.mp4` 时拒绝导出。
//!
//! 代理文件名沿用 `proxy::output_path` 的约定：`<原名>.proxy.mp4`，撞名时有 `(2)` 编号。

use super::{AssetId, MediaInfo, Project};
use std::path::{Path, PathBuf};

/// 代理文件名里夹的标记：`<原名>.proxy.mp4`。
pub const PROXY_MARK: &str = ".proxy";

/// 预览时实际要打开的文件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewFile {
    /// 原片。
    Original(PathBuf),
    /// 代理副本。
    Proxy(PathBuf),
}

impl PreviewFile {
    pub fn path(&self) -> &Path {
        match self {
            PreviewFile::Original(path) | PreviewFile::Proxy(path) => path,
        }
    }

    pub fn is_proxy(&self) -> bool {
        matches!(self, PreviewFile::Proxy(_))
    }
}

/// 路径是不是代理文件（按文件名里的 `.proxy` 标记判断，不碰文件系统）。
pub fn is_proxy_file(path: &Path) -> bool {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().contains(PROXY_MARK))
        .unwrap_or(false)
}

/// 找 `input` 对应的代理文件：先 `<原名>.proxy.mp4`，再 `(2)` 到 `(9)`。
///
/// `exists` 传进来而不是直接读盘，测试才能钉住「撞名编号」这条规则。
pub fn find_proxy(input: &Path, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let dir = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input.file_stem()?.to_string_lossy().into_owned();
    if stem.is_empty() {
        return None;
    }
    let first = dir.join(format!("{stem}{PROXY_MARK}.mp4"));
    if exists(&first) {
        return Some(first);
    }
    for index in 2..=9 {
        let candidate = dir.join(format!("{stem}{PROXY_MARK} ({index}).mp4"));
        if exists(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// 素材比序列大（任一边更长）才值得用代理：小素材套代理只会更糊。
pub fn proxy_worthwhile(media: &MediaInfo, width: u32, height: u32) -> bool {
    let (media_width, media_height) = media.display_size();
    media_width > width || media_height > height
}

/// 预览该用哪个文件：值得用代理、代理在、且原片本身不是代理时用代理。
///
/// `exists` 同 [`find_proxy`]，由调用方给。
pub fn preview_file(
    original: &Path,
    media: Option<&MediaInfo>,
    width: u32,
    height: u32,
    exists: impl Fn(&Path) -> bool,
) -> PreviewFile {
    if is_proxy_file(original) {
        // 用户自己导进来的就是代理：没有原片可退，只能照它播。
        return PreviewFile::Original(original.to_path_buf());
    }
    let worthwhile = media.is_some_and(|media| proxy_worthwhile(media, width, height));
    if worthwhile && let Some(proxy) = find_proxy(original, exists) {
        return PreviewFile::Proxy(proxy);
    }
    PreviewFile::Original(original.to_path_buf())
}

/// 工程里某个素材在预览时该用哪个文件。素材不在表里或路径为空时返回 `None`。
pub fn preview_for(project: &Project, asset: AssetId) -> Option<PreviewFile> {
    let asset = project.asset(asset)?;
    let original = project.asset_path(asset.id)?;
    let sequence = project.sequence();
    Some(preview_file(
        original,
        asset.media.as_ref(),
        sequence.width,
        sequence.height,
        |path| path.is_file(),
    ))
}

/// 这个素材要不要生成代理：有画面、比序列大、而且现在还没有代理。
///
/// 与 [`preview_for`] 的分工：那个回答「这一刻播哪个文件」，这个回答「该不该去做一份」。
/// 已经有代理的不再重排，避免批量补代理时反复盖同一个文件。
pub fn needs_proxy(project: &Project, asset: AssetId) -> bool {
    let Some(asset) = project.asset(asset) else {
        return false;
    };
    let Some(media) = asset.media.as_ref() else {
        return false;
    };
    let Some(original) = project.asset_path(asset.id) else {
        return false;
    };
    if is_proxy_file(original) || !media.has_video() {
        return false;
    }
    let sequence = project.sequence();
    proxy_worthwhile(media, sequence.width, sequence.height)
        && find_proxy(original, |path| path.is_file()).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{MediaInfo, SequenceSpec};

    fn media(width: u32, height: u32) -> MediaInfo {
        MediaInfo {
            width,
            height,
            ..MediaInfo::default()
        }
    }

    #[test]
    fn 代理文件名能认出来() {
        assert!(is_proxy_file(Path::new("C:/v/clip.proxy.mp4")));
        assert!(is_proxy_file(Path::new("C:/v/clip.proxy (2).mp4")));
        assert!(!is_proxy_file(Path::new("C:/v/clip.mp4")));
        // 只是名字里带 proxy 但不是我们的标记：不算。
        assert!(!is_proxy_file(Path::new("C:/v/myproxy.mp4")));
    }

    #[test]
    fn 找代理时先找无编号再找编号() {
        let input = Path::new("D:/media/take1.mov");
        let none: Vec<PathBuf> = Vec::new();
        assert_eq!(find_proxy(input, |_| false), None);
        let _ = &none;

        // 只有编号版本在。
        let second = PathBuf::from("D:/media/take1.proxy (2).mp4");
        let found = find_proxy(input, |path| path == second).expect("能找到编号版本");
        assert_eq!(found, second);

        // 无编号与编号都在：优先无编号。
        let first = PathBuf::from("D:/media/take1.proxy.mp4");
        let found = find_proxy(input, |path| path == first || path == second).expect("能找到");
        assert_eq!(found, first);
    }

    #[test]
    fn 素材比序列大才值得用代理() {
        let spec = SequenceSpec {
            width: 1920,
            height: 1080,
            ..SequenceSpec::default()
        };
        assert!(proxy_worthwhile(
            &media(3840, 2160),
            spec.width,
            spec.height
        ));
        assert!(
            proxy_worthwhile(&media(1920, 2160), spec.width, spec.height),
            "只高也值得"
        );
        assert!(!proxy_worthwhile(
            &media(1280, 720),
            spec.width,
            spec.height
        ));
        // 竖屏 1080×1920 对 1920×1080：高度更长，同样值得。
        assert!(proxy_worthwhile(
            &media(1080, 1920),
            spec.width,
            spec.height
        ));
    }

    #[test]
    fn 大素材有代理就用代理() {
        let original = Path::new("E:/media/4k.mov");
        let proxy = PathBuf::from("E:/media/4k.proxy.mp4");
        let file = preview_file(original, Some(&media(3840, 2160)), 1920, 1080, |path| {
            path == proxy
        });
        assert!(file.is_proxy(), "4K 素材对 1080p 序列，应该用代理");
        assert_eq!(file.path(), proxy);
    }

    #[test]
    fn 小素材或代理不在时用原片() {
        let original = Path::new("E:/media/hd.mov");
        // 小素材：即使代理在也不用。
        let file = preview_file(original, Some(&media(1280, 720)), 1920, 1080, |_| true);
        assert!(!file.is_proxy());
        assert_eq!(file.path(), original);

        // 大素材但代理不在：退回原片。
        let file = preview_file(original, Some(&media(3840, 2160)), 1920, 1080, |_| false);
        assert!(!file.is_proxy());

        // 没有素材信息时也退回原片。
        let file = preview_file(original, None, 1920, 1080, |_| true);
        assert!(!file.is_proxy());
    }

    #[test]
    fn 自己就是代理的素材不会被再套一层() {
        let original = Path::new("E:/media/4k.proxy.mp4");
        let file = preview_file(original, Some(&media(3840, 2160)), 1920, 1080, |_| true);
        assert!(!file.is_proxy());
        assert_eq!(file.path(), original);
    }

    #[test]
    fn 从工程里查预览文件会跳过缺失素材() {
        let mut project = Project::new();
        let path = PathBuf::from("E:/media/nothing.mov");
        let asset = project.add_asset(&path);
        // 文件不存在、也没有代理：退回原片路径（缺文件由预览层自己提示）。
        let file = preview_for(&project, asset).expect("素材已登记");
        assert!(!file.is_proxy());
        assert_eq!(file.path(), path);
        // 不存在的素材 id 返回 None。
        assert_eq!(preview_for(&project, AssetId::new(999)), None);
    }

    #[test]
    fn 该不该补代理() {
        let dir = std::env::temp_dir().join(format!("ngy-needs-proxy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("clip.mov");
        std::fs::write(&original, b"x").unwrap();

        let mut project = Project::new();
        let id = project.add_asset(&original);
        // 还没探测：不知道多大，不排。
        assert!(!needs_proxy(&project, id));

        // 4K 素材进 1080p 序列：该补。
        project.assets[0].media = Some(media(3840, 2160));
        assert!(needs_proxy(&project, id));

        // 已经有代理：不再重排（批量补代理不该反复盖同一个文件）。
        std::fs::write(dir.join("clip.proxy.mp4"), b"x").unwrap();
        assert!(!needs_proxy(&project, id));

        // 小素材不排。
        let small = dir.join("small.mov");
        std::fs::write(&small, b"x").unwrap();
        let small_id = project.add_asset(&small);
        let last = project.assets.len() - 1;
        project.assets[last].media = Some(media(1280, 720));
        assert!(!needs_proxy(&project, small_id));

        // 自己就是代理的不排：那是用户自己的素材，不能再派一份。
        let proxy_only = dir.join("other.proxy.mp4");
        std::fs::write(&proxy_only, b"x").unwrap();
        let proxy_id = project.add_asset(&proxy_only);
        let last = project.assets.len() - 1;
        project.assets[last].media = Some(media(3840, 2160));
        assert!(!needs_proxy(&project, proxy_id));

        // 没登记的 id 不排。
        assert!(!needs_proxy(&project, AssetId::new(999)));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
