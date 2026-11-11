//! 工程文件：往返、相对路径、缺失素材、备份与版本迁移。

use super::*;
use crate::timeline::ClipItem;
use std::fs;

#[path = "tests/sequences.rs"]
mod sequences;

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

    let video = project.sequence_mut().tracks[0].id;
    let clip = project.sequence_mut().alloc_id();
    project.sequence_mut().track_mut(video).unwrap().clips.push(
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

    let mut back = Project::open(&path).expect("打开工程");
    assert_eq!(back.version, PROJECT_VERSION);
    assert_eq!(back.sequence().name, project.sequence().name);
    assert_eq!(back.sequence_mut().fps, project.sequence_mut().fps);
    assert_eq!(back.sequence_mut().tracks[0].clips.len(), 1);
    assert_eq!(back.sequence_mut().tracks[0].clips[0].id, clip);
    assert_eq!(back.sequence_mut().tracks[0].clips[0].name, "开场");
    assert_eq!(back.sequence_mut().tracks[0].clips[0].asset_id, asset);
    assert_eq!(back.ui.playhead, Tick::from_millis(1500));
    assert_eq!(back.ui.selected, vec![clip]);
    assert!(back.sequence_mut().validate().is_ok());

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
fn 入出点成对才有效且顺序不对会清掉另一端() {
    let mut ui = UiState::default();
    assert_eq!(ui.range(), None);

    ui.set_in(Tick::from_secs(1.0), Tick::ZERO);
    assert_eq!(ui.range(), None, "只有入点不算区间");
    ui.set_out(Tick::from_secs(3.0), Tick::ZERO);
    assert_eq!(
        ui.range(),
        Some((Tick::from_secs(1.0), Tick::from_secs(3.0)))
    );

    // 同一时刻再按一次出点：清掉。
    ui.set_out(Tick::from_secs(3.0), Tick::ZERO);
    assert_eq!(ui.out_point, None);
    assert_eq!(ui.range(), None);

    // 出点早于入点：保留出点，入点被清掉，不留反向区间。
    ui.set_in(Tick::from_secs(4.0), Tick::ZERO);
    ui.set_out(Tick::from_secs(2.0), Tick::ZERO);
    assert_eq!(ui.in_point, None);
    assert_eq!(ui.out_point, Some(Tick::from_secs(2.0)));
}

#[test]
fn 入出点按像素容差清除() {
    let mut ui = UiState::default();
    // 8 像素/秒的默认缩放下，一个像素就是 125 毫秒。
    let tolerance = Tick::from_millis(125);
    ui.set_in(Tick::from_secs(2.0), tolerance);
    ui.set_out(Tick::from_secs(5.0), tolerance);

    // 播放头拖回来差 60 毫秒：算回到入点上，再按一次清掉。
    ui.set_in(Tick::from_millis(2060), tolerance);
    assert_eq!(ui.in_point, None, "同一像素内再按一次是清除");
    assert_eq!(ui.out_point, Some(Tick::from_secs(5.0)), "清入点不动出点");

    // 容差之外是改设新值，不该把已有入点清掉。
    ui.set_in(Tick::from_secs(1.0), tolerance);
    ui.set_in(Tick::from_millis(1900), tolerance);
    assert_eq!(ui.in_point, Some(Tick::from_millis(1900)));

    // 右键菜单的兜底入口：不看播放头，两个点一起清。
    ui.clear_range();
    assert_eq!(ui.range(), None);
    assert!(ui.in_point.is_none() && ui.out_point.is_none());
}

#[test]
fn 视图缩放随工程往返且老工程用默认() {
    let ui = UiState {
        zoom: Some(12.5),
        scroll: 3.25,
        ..UiState::default()
    };

    let text = serde_json::to_string(&ui).expect("序列化");
    let back: UiState = serde_json::from_str(&text).expect("反序列化");
    assert_eq!(back.zoom, Some(12.5));
    assert!((back.scroll - 3.25).abs() < 1e-9);

    // 老工程里没有这两个字段：读出来要能落到「按播放头居中」的分支。
    let legacy: UiState = serde_json::from_str("{}").expect("老工程");
    assert_eq!(legacy.zoom, None);
    assert_eq!(legacy.scroll, 0.0);
}

#[test]
fn 内容比对忽略播放头但认得序列改动() {
    let dir = scratch("same-content");
    let (project, _, _, _) = sample(&dir);
    let mut other = project.clone();
    assert!(project.same_content(&other), "刚克隆出来当然一样");

    other.ui.playhead = Tick::from_secs(3.0);
    other.ui.selected.clear();
    assert!(project.same_content(&other), "拖播放头、改选中不算改动");

    other.sequence_mut().name = "改名了".to_string();
    assert!(!project.same_content(&other), "序列名改了要算改动");

    let mut third = project.clone();
    third.assets[0].path = PathBuf::from("别处/a.mp4");
    assert!(!project.same_content(&third), "素材路径改了要算改动");

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
    project.sequence_mut().name = "第一版".to_string();
    project.save(&path).expect("第一次保存");

    project.sequence_mut().name = "第二版".to_string();
    project.save(&path).expect("第二次保存");

    let current = Project::from_json(&fs::read_to_string(&path).expect("读当前工程")).unwrap();
    assert_eq!(current.sequence().name, "第二版");

    let backup = Project::from_json(&fs::read_to_string(bak_of(&path)).expect("读备份")).unwrap();
    assert_eq!(
        backup.sequence().name,
        "第一版",
        ".bak 是上一次保存的那一版"
    );

    let temp = dir.join("p.ngyproj.tmp");
    assert!(!temp.exists(), "临时文件不能留在磁盘上");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 连续保存两次内容不变() {
    let dir = scratch("idempotent");
    let path = dir.join("p.ngyproj");
    let (mut project, _, _, _) = sample(&dir);
    project.save(&path).expect("第一次保存");
    let after_first = project.clone();
    project.save(&path).expect("第二次保存");

    // 保存会把素材路径重写成相对形式。第二次保存要是又改了内容，
    // 界面上的「未保存」标记会在每次 Ctrl+S 之后立刻亮回来。
    assert!(
        after_first.same_content(&project),
        "同一份工程再存一次不该变内容"
    );

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
    assert_eq!(project.sequence().name, "旧工程");
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
fn 探测结果随工程存盘且文件改动后标记过期() {
    let dir = scratch("media-cache");
    let path = dir.join("p.ngyproj");
    let file = media(&dir, "a.mp4");
    let mut project = Project::new();
    let asset = project.add_asset(&file);

    let stamp = MediaStamp::of(&file).expect("文件指纹");
    {
        let entry = project
            .assets
            .iter_mut()
            .find(|entry| entry.id == asset)
            .expect("素材登记");
        entry.media = Some(MediaInfo {
            duration: Tick::from_secs(10.0),
            width: 1920,
            height: 1080,
            channels: 2,
            ..MediaInfo::default()
        });
        entry.stamp = Some(stamp);
    }
    assert_eq!(project.asset(asset).unwrap().state(), AssetState::Ready);

    project.save(&path).expect("保存工程");
    let project = Project::open(&path).expect("打开工程");
    let entry = project.asset(asset).expect("素材表还在");
    assert_eq!(
        entry.media.as_ref().unwrap().width,
        1920,
        "探测结果随工程存盘"
    );
    assert_eq!(entry.state(), AssetState::Ready);

    // 换一份内容不同、大小也不同的文件：缓存必须失效而不是静默沿用。
    fs::write(&file, b"a completely different file").expect("换文件");
    assert_eq!(entry.state(), AssetState::Changed);

    fs::remove_file(&file).expect("删文件");
    let reopened = Project::open(&path).expect("素材没了也能打开");
    assert_eq!(
        reopened.asset(asset).unwrap().state(),
        AssetState::Missing,
        "文件不在原地时提示重链"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 保存会去掉指向已删片段的转场() {
    let dir = scratch("dangling");
    let path = dir.join("p.ngyproj");
    let mut project = Project::new();
    let video = project.sequence_mut().tracks[0].id;
    project
        .sequence_mut()
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
    assert!(back.sequence().tracks[0].transitions.is_empty());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 同一路径重复导入只登记一次() {
    let dir = scratch("dedupe");
    let file = media(&dir, "a.mp4");
    let mut project = Project::new();

    let first = project.add_asset(&file);
    let again = project.add_asset(&file);
    assert_eq!(first, again, "同一路径必须复用同一条素材");
    assert_eq!(project.assets.len(), 1);

    // 相对路径写法不同但指向同一个文件，也要认出来。
    let relative = file
        .strip_prefix(std::env::current_dir().unwrap_or_default())
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| file.clone());
    assert_eq!(project.add_asset(&relative), first, "写法不同不该新增一条");
    assert_eq!(project.assets.len(), 1);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 被片段引用的素材不能移除() {
    let dir = scratch("remove-in-use");
    let (mut project, file, asset, _) = sample(&dir);

    assert_eq!(project.asset_clip_count(asset), 1);
    assert_eq!(
        project.remove_asset(asset),
        Err(AssetRemoveError::InUse { clips: 1 }),
        "删掉会留下悬空引用"
    );
    assert!(project.asset(asset).is_some(), "拒绝移除时素材必须还在");

    // 把引用它的片段删掉之后就能移除，文件本身不受影响。
    project.sequence_mut().tracks[0].clips.clear();
    assert_eq!(project.remove_asset(asset), Ok(()));
    assert!(project.asset(asset).is_none());
    assert!(file.is_file(), "只改素材表，不动磁盘上的文件");

    assert_eq!(
        project.remove_asset(asset),
        Err(AssetRemoveError::NotFound(asset)),
        "重复移除报「没有这一条」而不是静默成功"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 字幕条目随工程往返且老工程缺字段也能打开() {
    use crate::timeline::{COLORS, SubtitleItem, SubtitlePosition, TrackKind};

    let dir = scratch("subtitle-roundtrip");
    let path = dir.join("p.ngyproj");
    let (mut project, _, _, _) = sample(&dir);
    let track = project.sequence_mut().add_track(TrackKind::Subtitle);
    let id = project.sequence_mut().alloc_id();
    let mut entry = SubtitleItem::new(id, Tick::from_secs(5.0), Tick::from_secs(8.0), "你好，世界");
    entry.style.size = 64;
    entry.style.color = COLORS[1].0;
    entry.style.position = SubtitlePosition::Top;
    project
        .sequence_mut()
        .track_mut(track)
        .unwrap()
        .subtitles
        .push(entry.clone());

    project.save(&path).expect("保存工程");
    let mut back = Project::open(&path).expect("打开工程");
    assert_eq!(back.sequence().subtitle(id), Some(&entry));
    assert_eq!(back.sequence_mut().duration(), Tick::from_secs(8.0));
    assert_eq!(back.sequence_mut().subtitle_track(id), Some(track));
    assert!(back.sequence_mut().validate().is_ok());

    // 这一版之前写的工程没有 `subtitles` 键：整条字幕轨仍要能打开。
    let legacy = r#"{"version":1,"sequence":{"tracks":[{"id":1,"kind":"subtitle",
        "name":"字幕 1","clips":[],"transitions":[],"muted":false,"locked":false}],
        "markers":[]},"assets":[],"ui":{},"next_asset_id":1}"#;
    let old = Project::from_json(legacy).expect("老工程照常打开");
    assert!(old.sequence().tracks[0].subtitles.is_empty());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 打开工程之后文件被删掉也报缺失() {
    let dir = scratch("vanish");
    let (mut project, file, asset, _) = sample(&dir);
    {
        let entry = project
            .assets
            .iter_mut()
            .find(|entry| entry.id == asset)
            .expect("素材登记");
        entry.media = Some(MediaInfo {
            duration: Tick::from_secs(1.0),
            ..MediaInfo::default()
        });
        entry.stamp = MediaStamp::of(&file);
    }
    assert_eq!(project.asset(asset).unwrap().state(), AssetState::Ready);

    // 文件在会话期间被删掉：要报「不在了」，而不是含糊的「改动过」。
    fs::remove_file(&file).expect("删文件");
    assert_eq!(project.asset(asset).unwrap().state(), AssetState::Missing);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 移除素材不影响其它素材与片段() {
    let dir = scratch("remove-other");
    let (mut project, _, used, _) = sample(&dir);
    let other = project.add_asset(&media(&dir, "b.mp4"));

    assert_eq!(project.remove_asset(other), Ok(()));
    assert!(project.asset(used).is_some(), "用着的那个原样留着");
    assert_eq!(project.asset_clip_count(used), 1);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn lut_路径随工程相对化并在另存后跟着新目录() {
    let dir = scratch("lut-path");
    let (mut project, _, _, clip) = sample(&dir);
    let lut = media(&dir, "luts/warm.cube");
    project.sequence_mut().clip_mut(clip).unwrap().color.lut = Some(lut.clone());

    let path = dir.join("p.ngyproj");
    project.save(&path).expect("保存工程");
    let text = fs::read_to_string(&path).expect("读工程");
    assert!(
        !text.contains(&dir.to_string_lossy().replace('\\', "\\\\")),
        "LUT 该写相对路径：{text}"
    );

    // 内存里也跟着变成便携形式，导出时由 `resolve_path` 还原。
    let stored = project
        .sequence()
        .clip(clip)
        .unwrap()
        .color
        .lut
        .clone()
        .expect("LUT 还在");
    assert!(stored.is_relative(), "{stored:?}");
    assert!(project.resolve_path(&stored).is_file());

    // 再存一次内容不变：否则每次 Ctrl+S 都会把「未保存」重新点亮。
    let before = project.clone();
    project.save(&path).expect("再存一次");
    assert!(before.same_content(&project));

    // 另存到子目录：LUT 不在新目录下面，相对路径表示不了（`..` 不写进工程，
    // 与素材同一条规则），退回绝对路径——但必须还指着同一个文件。
    let sub = dir.join("sub");
    fs::create_dir_all(&sub).expect("建子目录");
    project.save(&sub.join("p.ngyproj")).expect("另存为");
    let stored = project
        .sequence()
        .clip(clip)
        .unwrap()
        .color
        .lut
        .clone()
        .unwrap();
    assert!(stored.is_absolute(), "{stored:?}");
    assert!(project.resolve_path(&stored).is_file(), "{stored:?}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 相对_lut_路径按工程目录解析() {
    let dir = scratch("lut-relative");
    let (mut project, _, _, clip) = sample(&dir);
    project.sequence_mut().clip_mut(clip).unwrap().color.lut = Some(PathBuf::from("luts/a.cube"));

    // 没保存过的工程没有基准目录：相对路径原样返回。
    assert_eq!(
        project.resolve_path(Path::new("luts/a.cube")),
        PathBuf::from("luts/a.cube")
    );

    project.save(&dir.join("p.ngyproj")).expect("保存工程");
    assert_eq!(
        project.resolve_path(Path::new("luts/a.cube")),
        dir.join("luts/a.cube")
    );

    let _ = fs::remove_dir_all(&dir);
}
