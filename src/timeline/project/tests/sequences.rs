//! 多序列：老工程迁移、往返、增删改、切换复位与读盘自愈。

use super::*;
use std::collections::HashSet;

fn spec() -> SequenceSpec {
    SequenceSpec::new(1920, 1080, Rational::FPS_30)
}

#[test]
fn 老工程读成只有一条序列且补上_id() {
    let legacy = r#"{"version":1,"sequence":{"name":"旧工程","tracks":[],"markers":[]},
        "assets":[],"ui":{},"next_asset_id":1}"#;
    let project = Project::from_json(legacy).expect("老工程能打开");
    assert_eq!(project.version, PROJECT_VERSION, "读进来就升到当前版本");
    assert_eq!(project.sequences().len(), 1);
    assert_eq!(project.sequence().name, "旧工程");
    assert_eq!(project.sequence().id.raw(), 1);
    assert_eq!(project.active_id(), project.sequence().id);
}

#[test]
fn 没有版本号的工程也按老格式迁移() {
    let legacy = r#"{"sequence":{"name":"无版本号"},"assets":[],"ui":{}}"#;
    let project = Project::from_json(legacy).expect("没版本号也能打开");
    assert_eq!(project.sequences().len(), 1);
    assert_eq!(project.sequence().name, "无版本号");
}

#[test]
fn 多序列往返后列表与当前指针都恢复() {
    let dir = scratch("sequences-roundtrip");
    let path = dir.join("p.ngyproj");
    let (mut project, _file, _asset, _clip) = sample(&dir);
    let second = project.add_sequence("采访", spec());
    project.rename_sequence(second, "采访 B").expect("改名成功");
    project.switch_sequence(second).expect("切换成功");
    project.ui.playhead = Tick::from_secs(2.0);
    project.save(&path).expect("保存工程");

    let back = Project::open(&path).expect("打开工程");
    assert_eq!(back.sequences().len(), 2);
    assert_eq!(back.active_id(), second, "下次打开停在同一条序列");
    assert_eq!(back.sequence().name, "采访 B");
    assert_eq!(back.ui.playhead, Tick::from_secs(2.0));
    assert!(back.same_content(&project), "落盘内容没被改写");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 切换序列复位播放头选中与工作区() {
    let mut project = Project::new();
    let second = project.add_sequence("B", spec());
    project.ui.playhead = Tick::from_secs(9.0);
    project.ui.selected = vec![Id::new(7)];
    project.ui.in_point = Some(Tick::from_secs(1.0));
    project.ui.out_point = Some(Tick::from_secs(3.0));

    project.switch_sequence(second).expect("切换成功");
    assert_eq!(project.active_id(), second);
    assert_eq!(project.ui.playhead, Tick::ZERO);
    assert!(project.ui.selected.is_empty(), "旧序列的 id 不能带过去");
    assert_eq!(project.ui.range(), None, "工作区是位置，不是内容");
    assert!(
        project.switch_sequence(second).is_ok(),
        "切到当前序列是空操作，不该再复位一次"
    );
}

#[test]
fn 最后一条不能删_删当前自动切到第一条() {
    let mut project = Project::new();
    let first = project.active_id();
    assert_eq!(
        project.remove_sequence(first),
        Err(SequenceError::IsLast),
        "工程至少要有一条时间线"
    );
    let second = project.add_sequence("B", spec());
    project.switch_sequence(second).unwrap();
    project.ui.playhead = Tick::from_secs(4.0);
    project.remove_sequence(second).expect("删除成功");
    assert_eq!(project.sequences().len(), 1);
    assert_eq!(project.active_id(), first);
    assert_eq!(project.ui.playhead, Tick::ZERO);

    assert_eq!(
        project.remove_sequence(Id::new(999)),
        Err(SequenceError::NotFound(Id::new(999)))
    );
    assert_eq!(
        project.rename_sequence(first, "  "),
        Err(SequenceError::EmptyName)
    );
    project
        .rename_sequence(first, "  成片  ")
        .expect("改名成功");
    assert_eq!(project.sequence().name, "成片", "两头空格要去掉");
}

#[test]
fn 新建序列的_id_不与已有序列撞() {
    let mut project = Project::new();
    let ids: Vec<Id> = (0..3).map(|_| project.add_sequence("", spec())).collect();
    let unique: HashSet<Id> = ids.iter().copied().collect();
    assert_eq!(unique.len(), 3);
    let all: HashSet<Id> = project
        .sequences()
        .iter()
        .map(|sequence| sequence.id)
        .collect();
    assert_eq!(all.len(), 4, "新建工程自带一条，再加三条");
    assert!(ids.iter().all(|id| all.contains(id)));
    assert_eq!(project.sequences()[1].name, "序列 2", "空名字自动编号");
}

#[test]
fn 手改工程的空列表_重复id_坏指针都能打开() {
    let empty = r#"{"version":2,"sequences":[],"active_sequence":99,"assets":[],"ui":{}}"#;
    let project = Project::from_json(empty).expect("空列表能自愈");
    assert_eq!(project.sequences().len(), 1);
    assert_eq!(project.active_id(), project.sequences()[0].id);

    let dup = r#"{"version":2,"sequences":[{"id":3,"name":"A"},{"id":3,"name":"B"}],
        "active_sequence":99,"assets":[],"ui":{}}"#;
    let project = Project::from_json(dup).expect("重复 id 能自愈");
    assert_eq!(project.sequences().len(), 2);
    assert_ne!(project.sequences()[0].id, project.sequences()[1].id);
    assert_eq!(project.active_id(), project.sequences()[0].id);
}

#[test]
fn 素材被任一序列引用时不能删() {
    let dir = scratch("sequences-assets");
    let file = media(&dir, "media/a.mp4");
    let mut project = Project::new();
    let asset = project.add_asset(&file);
    let second = project.add_sequence("B", spec());
    project.switch_sequence(second).unwrap();

    // 只有第二条序列用到这个素材：素材表是全工程共享的，仍要拒绝删除。
    let track = project.sequence().tracks[0].id;
    let clip = project.sequence_mut().alloc_id();
    project
        .sequence_mut()
        .track_mut(track)
        .unwrap()
        .clips
        .push(ClipItem::new(
            clip,
            asset,
            Tick::ZERO,
            Tick::from_secs(1.0),
            Tick::ZERO,
        ));
    assert_eq!(project.asset_clip_count(asset), 1);
    assert_eq!(
        project.remove_asset(asset),
        Err(AssetRemoveError::InUse { clips: 1 })
    );
    let _ = fs::remove_dir_all(&dir);
}
