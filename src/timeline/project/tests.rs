//! 工程文件：往返、相对路径、缺失素材、备份与版本迁移。

use super::*;
use crate::timeline::ClipItem;
use std::fs;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ngy-vp-project-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("创建临时目录");
    dir
}

fn media(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("创建素材目录");
    }
    fs::write(&path, b"fake media").expect("写素材文件");
    path
}

fn bak_of(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".bak");
    PathBuf::from(name)
}

/// 建一个带素材、一个片段、播放头和选中的工程。
fn sample(dir: &Path) -> (Project, PathBuf, AssetId, Id) {
    let file = media(dir, "media/a.mp4");
    let mut project = Project::new();
    let asset = project.add_asset(&file);

    let video = project.sequence.tracks[0].id;
    let clip = project.sequence.alloc_id();
    project.sequence.track_mut(video).unwrap().clips.push(
        ClipItem::new(
            clip,
            asset,
            Tick::from_micros(1_000_000),
            Tick::from_micros(5_000_000),
            Tick::ZERO,
        )
        .with_name("开场"),
    );
    project.ui.playhead = Tick::from_millis(1500);
    project.ui.selected = vec![clip];

    (project, file, asset, clip)
}

#[test]
fn 保存后能原样打开() {
    let dir = scratch("roundtrip");
    let path = dir.join("p.ngyproj");
    let (mut project, file, asset, clip) = sample(&dir);

    project.save(&path).expect("保存工程");

    let back = Project::open(&path).expect("打开工程");
    assert_eq!(back.version, PROJECT_VERSION);
    assert_eq!(back.sequence.name, project.sequence.name);
    assert_eq!(back.sequence.fps, project.sequence.fps);
    assert_eq!(back.sequence.tracks[0].clips.len(), 1);
    assert_eq!(back.sequence.tracks[0].clips[0].id, clip);
    assert_eq!(back.sequence.tracks[0].clips[0].name, "开场");
    assert_eq!(back.sequence.tracks[0].clips[0].asset_id, asset);
    assert_eq!(back.ui.playhead, Tick::from_millis(1500));
    assert_eq!(back.ui.selected, vec![clip]);
    assert!(back.sequence.validate().is_ok());

    assert_eq!(back.assets.len(), 1);
    assert!(!back.assets[0].missing, "素材还在原地");
    assert_eq!(back.assets[0].display_name(), "a.mp4");
    let resolved = back.asset_path(asset).expect("能算出素材路径");
    assert!(resolved.is_file());
    assert_eq!(resolved, file);
    assert_eq!(back.base_dir(), dir);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 素材路径相对工程文件优先() {
    let dir = scratch("relative");
    let path = dir.join("p.ngyproj");
    let (mut project, _, _, _) = sample(&dir);
    project.save(&path).expect("保存工程");

    // 文件里存的是 `media/a.mp4`，不是 `<tmp>/.../media/a.mp4`。
    let back = Project::from_json(&fs::read_to_string(&path).expect("读工程")).expect("解析");
    assert_eq!(back.assets[0].path, Path::new("media").join("a.mp4"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 跨目录的素材退回绝对路径() {
    let dir = scratch("absolute");
    let outside = scratch("absolute-outside");
    let path = dir.join("p.ngyproj");
    let file = media(&outside, "b.mp4");

    let mut project = Project::new();
    project.add_asset(&file);
    project.save(&path).expect("保存工程");

    let back = Project::from_json(&fs::read_to_string(&path).expect("读工程")).expect("解析");
    assert!(
        back.assets[0].path.is_absolute(),
        "跨目录树时只能写绝对路径：{:?}",
        back.assets[0].path
    );

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&outside);
}

#[test]
fn 素材缺失时工程照常打开并打标记() {
    let dir = scratch("missing");
    let path = dir.join("p.ngyproj");
    let (mut project, file, asset, _) = sample(&dir);
    project.save(&path).expect("保存工程");

    fs::remove_file(&file).expect("删掉素材");
    let back = Project::open(&path).expect("素材没了也要能打开");
    assert!(back.assets[0].missing);
    assert!(
        back.asset_path(asset).unwrap().ends_with("a.mp4"),
        "仍然能告诉用户它原来在哪儿"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 同一个文件重复导入只登记一次() {
    let dir = scratch("dedupe");
    let file = media(&dir, "a.mp4");
    let mut project = Project::new();

    let first = project.add_asset(&file);
    let second = project.add_asset(&file);
    assert_eq!(first, second);
    assert_eq!(project.assets.len(), 1);

    // 换一种写法指向同一个文件。
    let same = dir.join(".").join("a.mp4");
    assert_eq!(project.add_asset(&same), first);
    assert_eq!(project.assets.len(), 1);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 保存时留下上一版备份() {
    let dir = scratch("backup");
    let path = dir.join("p.ngyproj");
    let mut project = Project::new();
    project.sequence.name = "第一版".to_string();
    project.save(&path).expect("第一次保存");

    project.sequence.name = "第二版".to_string();
    project.save(&path).expect("第二次保存");

    let current = Project::from_json(&fs::read_to_string(&path).expect("读当前工程")).unwrap();
    assert_eq!(current.sequence.name, "第二版");

    let backup = Project::from_json(&fs::read_to_string(bak_of(&path)).expect("读备份")).unwrap();
    assert_eq!(backup.sequence.name, "第一版", ".bak 是上一次保存的那一版");

    let temp = dir.join("p.ngyproj.tmp");
    assert!(!temp.exists(), "临时文件不能留在磁盘上");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 保存之后内存里的素材路径仍然可用() {
    let dir = scratch("after-save");
    let path = dir.join("p.ngyproj");
    let (mut project, _, asset, _) = sample(&dir);

    project.save(&path).expect("保存工程");
    assert!(!project.assets[0].missing);
    assert!(
        project.asset_path(asset).unwrap().is_file(),
        "保存后立刻继续用，不该变成空路径"
    );

    // 再存一次也不该把相对路径写成绝对路径。
    project.save(&path).expect("再存一次");
    let text = fs::read_to_string(&path).expect("读工程");
    assert!(
        !text.contains(&dir.to_string_lossy().replace('\\', "\\\\")),
        "{text}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 老工程没有版本号也能读() {
    let project =
        Project::from_json(r#"{"sequence":{"name":"旧工程"}}"#).expect("没有 version 当作 v1");
    assert_eq!(project.version, PROJECT_VERSION);
    assert_eq!(project.sequence.name, "旧工程");
    assert!(project.assets.is_empty());
}

#[test]
fn 坏文件与过新版本给出明确错误() {
    assert!(matches!(
        Project::from_json("{"),
        Err(ProjectError::Parse { .. })
    ));
    assert!(matches!(
        Project::from_json("[]"),
        Err(ProjectError::NotAProject)
    ));
    match Project::from_json(r#"{"version": 99}"#) {
        Err(ProjectError::TooNew { version }) => assert_eq!(version, 99),
        other => panic!("应当拒绝过新版本：{other:?}"),
    }
}

#[test]
fn 加载时修好计数器并剔除失效选中() {
    let text = r#"{
        "version": 1,
        "assets": [{"id": 7, "path": "a.mp4"}],
        "ui": {"selected": [1, 2]}
    }"#;
    let mut project = Project::from_json(text).expect("解析工程");
    assert_eq!(
        project.alloc_asset_id(),
        AssetId::new(8),
        "素材 id 接着最大值"
    );
    assert!(
        project.ui.selected.is_empty(),
        "这些片段已经不存在了：{:?}",
        project.ui.selected
    );
}

#[test]
fn 保存会去掉指向已删片段的转场() {
    let dir = scratch("dangling");
    let path = dir.join("p.ngyproj");
    let mut project = Project::new();
    let video = project.sequence.tracks[0].id;
    project
        .sequence
        .track_mut(video)
        .unwrap()
        .transitions
        .push(crate::timeline::Transition::new(
            Id::new(1),
            Id::new(2),
            Id::new(3),
            crate::timeline::TransitionKind::Fade,
            Tick::from_millis(200),
        ));

    project.save(&path).expect("保存工程");
    let back = Project::open(&path).expect("打开工程");
    assert!(back.sequence.tracks[0].transitions.is_empty());

    let _ = fs::remove_dir_all(&dir);
}
