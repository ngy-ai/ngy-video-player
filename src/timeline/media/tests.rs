//! 素材信息：旋转元数据、文件指纹与状态文案。

use super::*;
use std::fs;

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ngy-vp-media-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("创建临时目录");
    dir
}

#[test]
fn 竖拍素材按旋转元数据换显示尺寸() {
    let info = MediaInfo {
        width: 1920,
        height: 1080,
        rotation: 90,
        ..MediaInfo::default()
    };
    assert_eq!(info.display_size(), (1080, 1920));

    let upside_down = MediaInfo {
        rotation: 270,
        ..info.clone()
    };
    assert_eq!(upside_down.display_size(), (1080, 1920));

    let normal = MediaInfo {
        rotation: 180,
        ..info
    };
    assert_eq!(normal.display_size(), (1920, 1080));
}

#[test]
fn 指纹能发现文件被改过() {
    let dir = scratch("stamp");
    let file = dir.join("a.mp4");
    fs::write(&file, b"one").expect("写文件");
    let first = MediaStamp::of(&file).expect("读指纹");
    assert_eq!(first.size, 3);

    fs::write(&file, b"much longer content").expect("改文件");
    let second = MediaStamp::of(&file).expect("读指纹");
    assert_ne!(first, second, "大小变了就不能继续用旧缓存");

    assert_eq!(
        MediaStamp::of(&dir.join("gone.mp4")),
        None,
        "文件不在就没有指纹"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 三种素材状态都给出了下一步() {
    assert_eq!(AssetState::Ready.message(), "可用");
    assert!(AssetState::Missing.message().contains("重新链接"));
    assert!(AssetState::Changed.message().contains("重新探测"));
}
