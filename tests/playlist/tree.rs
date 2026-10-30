//! 树形列表：递归展开、折叠、拖放移动与层级存档。
//!
//! 这里的列表大多直接用存档 JSON 构造：`depth` 没有公开的 setter，
//! 而「一棵已经长好的树」正是这些用例的输入。真实目录只用来验递归扫描。

use super::{names, sample, scratch, touch};
use ngy_video_player::playlist::{DropSpot, Playlist, PlaylistSet, SortMode, collect_tree};
use std::fs;
use std::path::PathBuf;

fn dir_item(path: &str, depth: u16) -> String {
    format!(r#"{{"path":"{path}","dir":true,"depth":{depth}}}"#)
}

fn file_item(path: &str, depth: u16) -> String {
    format!(r#"{{"path":"{path}","depth":{depth}}}"#)
}

fn tree_set(items: &[String], index: usize) -> PlaylistSet {
    let text = format!(
        r#"{{"tabs":[{{"name":"t","playlist":{{"items":[{}],"index":{index}}}}}],"active":0}}"#,
        items.join(",")
    );
    PlaylistSet::from_json(&text)
}

fn tree_of(items: &[String], index: usize) -> Playlist {
    tree_set(items, index).tab(0).cloned().expect("第一页")
}

fn depths(playlist: &Playlist) -> Vec<u16> {
    playlist.items().iter().map(|item| item.depth()).collect()
}

fn mkdir(parent: &std::path::Path, name: &str) -> PathBuf {
    let path = parent.join(name);
    fs::create_dir_all(&path).expect("创建子目录");
    path
}

#[test]
fn 目录展开递归收进子文件夹() {
    let dir = scratch("tree-recursive");
    touch(&dir, "a.mp4");
    let sub = mkdir(&dir, "sub");
    touch(&sub, "b.mp4");

    let items = collect_tree(&dir);
    let names: Vec<String> = items
        .iter()
        .map(|item| {
            item.path()
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    let depths: Vec<u16> = items.iter().map(|item| item.depth()).collect();

    let root = dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    // 文件夹在前、文件在后；子文件夹连同里面的文件一起收进来。
    assert_eq!(
        names,
        [
            root,
            "sub".to_string(),
            "b.mp4".to_string(),
            "a.mp4".to_string()
        ]
    );
    assert_eq!(depths, [0, 1, 2, 1]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 一个能播的都没有的文件夹不留空壳() {
    let dir = scratch("tree-empty");
    touch(&dir, "a.mp4");
    mkdir(&dir, "empty");
    let nested = mkdir(&dir, "only-txt");
    touch(&nested, "note.txt");

    let items = collect_tree(&dir);
    assert_eq!(items.len(), 2, "根文件夹 + a.mp4");
    assert!(items.iter().all(|item| !item.is_dir() || item.depth() == 0));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 重复添加同一个目录只加一次() {
    let dir = scratch("tree-append-twice");
    touch(&dir, "a.mp4");

    let mut playlist = Playlist::default();
    assert_eq!(playlist.append_paths(std::slice::from_ref(&dir)), 1);
    let rows = playlist.len();
    assert_eq!(playlist.append_paths(std::slice::from_ref(&dir)), 0);
    assert_eq!(playlist.len(), rows, "第二次不该多出一个空壳文件夹");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 折叠的文件夹把子项一起藏起来() {
    let mut playlist = tree_of(
        &[
            dir_item("d", 0),
            file_item("d/a.mp4", 1),
            file_item("b.mp4", 0),
        ],
        1,
    );

    assert_eq!(playlist.visible_indices(), [0, 1, 2]);
    assert_eq!(playlist.dir_file_count(0), 1);
    assert!(playlist.toggle_dir(0));
    assert_eq!(playlist.visible_indices(), [0, 2]);
    assert!(playlist.items()[0].collapsed());
}

#[test]
fn 一键收起与展开所有文件夹() {
    let mut playlist = tree_of(
        &[
            dir_item("a", 0),
            dir_item("a/b", 1),
            file_item("a/b/x.mp4", 2),
            dir_item("c", 0),
            file_item("c/y.mp4", 1),
        ],
        2,
    );

    assert!(playlist.has_dir());
    assert!(!playlist.all_collapsed());

    assert!(playlist.set_all_collapsed(true));
    assert!(playlist.all_collapsed());
    assert_eq!(
        playlist.visible_indices(),
        [0, 3],
        "只剩两个顶层文件夹，里面全收"
    );

    // 已经是全收起了：再点一次不该算「有变化」。
    assert!(!playlist.set_all_collapsed(true));

    assert!(playlist.set_all_collapsed(false));
    assert!(!playlist.all_collapsed());
    assert_eq!(playlist.visible_indices().len(), 5);
}

#[test]
fn 没有文件夹时没有可收起的() {
    let mut playlist = sample(&["a.mp4", "b.mp4"], 0);
    assert!(!playlist.has_dir());
    assert!(
        !playlist.all_collapsed(),
        "没有文件夹不该显示成「已经全部收起」"
    );
    assert!(!playlist.set_all_collapsed(true));
}

#[test]
fn 下一集跳过折叠的文件夹() {
    let mut playlist = tree_of(
        &[
            dir_item("d", 0),
            file_item("d/a.mp4", 1),
            file_item("b.mp4", 0),
        ],
        1,
    );

    // 展开时 a 后面是 b；b 之后回绕到 a。
    assert_eq!(playlist.next_index(false), Some(2));
    playlist.set_index(2);
    assert_eq!(playlist.next_index(true), Some(1));

    // 收起后 a 不再参与播放，b 之后只能回到自己。
    playlist.toggle_dir(0);
    assert_eq!(playlist.next_index(true), Some(2));
}

#[test]
fn 拖进文件夹成为子项() {
    let mut playlist = tree_of(
        &[
            dir_item("d", 0),
            file_item("d/a.mp4", 1),
            file_item("d/b.mp4", 1),
            file_item("c.mp4", 0),
        ],
        1,
    );

    assert!(playlist.move_to(3, DropSpot::Into(0)));
    assert_eq!(names(&playlist), ["d", "d/a.mp4", "d/b.mp4", "c.mp4"]);
    assert_eq!(depths(&playlist), [0, 1, 1, 1]);
    assert_eq!(playlist.sort_mode(), SortMode::Manual);
    assert!(!playlist.items()[0].collapsed(), "拖进去之后文件夹要展开");
}

#[test]
fn 拖到文件夹后面插在整棵子树之后() {
    let mut playlist = tree_of(
        &[
            file_item("a.mp4", 0),
            dir_item("d", 0),
            file_item("d/x.mp4", 1),
        ],
        0,
    );

    assert!(playlist.move_to(0, DropSpot::After(1)));
    assert_eq!(names(&playlist), ["d", "d/x.mp4", "a.mp4"]);
    assert_eq!(
        depths(&playlist),
        [0, 1, 0],
        "和文件夹平级，而不是成为它的子项"
    );
}

#[test]
fn 从文件夹里拖出来变成平级() {
    let mut playlist = tree_of(
        &[
            file_item("e.mp4", 0),
            dir_item("d", 0),
            file_item("d/a.mp4", 1),
        ],
        2,
    );

    assert!(playlist.move_to(2, DropSpot::Before(0)));
    assert_eq!(names(&playlist), ["d/a.mp4", "e.mp4", "d"]);
    assert_eq!(depths(&playlist), [0, 0, 0]);
    assert_eq!(playlist.index(), Some(0), "当前项跟着文件走");
}

#[test]
fn 文件夹不能拖进自己的子树() {
    let mut playlist = tree_of(
        &[
            dir_item("d", 0),
            file_item("d/a.mp4", 1),
            file_item("b.mp4", 0),
        ],
        1,
    );

    assert!(!playlist.move_to(0, DropSpot::Into(0)));
    assert!(!playlist.move_to(0, DropSpot::After(0)));
    assert!(!playlist.move_to(1, DropSpot::Into(2)), "目标不是文件夹");
    assert_eq!(names(&playlist), ["d", "d/a.mp4", "b.mp4"]);
}

#[test]
fn 删除文件夹连带子项() {
    let mut playlist = tree_of(
        &[
            dir_item("d", 0),
            file_item("d/a.mp4", 1),
            file_item("d/b.mp4", 1),
            file_item("c.mp4", 0),
        ],
        3,
    );

    let removal = playlist.remove(0);
    assert_eq!(removal.removed, 3);
    assert_eq!(removal.removed_files, 2);
    assert_eq!(names(&playlist), ["c.mp4"]);
    assert_eq!(playlist.index(), Some(0));
}

#[test]
fn 排序只在同层里进行() {
    let mut playlist = tree_of(
        &[
            file_item("c.mp4", 0),
            file_item("b.mp4", 0),
            file_item("z.mp4", 0),
            dir_item("d", 0),
            file_item("d/y.mp4", 1),
            file_item("d/x.mp4", 1),
        ],
        0,
    );

    assert!(playlist.sort_items(SortMode::Name));
    // 顶层文件夹在前，然后按自然序；子层跟着父项一起走，并在自己那层排好。
    assert_eq!(
        names(&playlist),
        ["d", "d/x.mp4", "d/y.mp4", "b.mp4", "c.mp4", "z.mp4"]
    );
    assert_eq!(depths(&playlist), [0, 1, 1, 0, 0, 0]);
}

#[test]
fn 存档往返保留树结构与折叠状态() {
    let set = tree_set(
        &[
            dir_item("d", 0),
            file_item("d/a.mp4", 1),
            dir_item("d/sub", 1),
            file_item("d/sub/b.mp4", 2),
            file_item("c.mp4", 0),
        ],
        1,
    );
    let text = serde_json::to_string(&set).expect("序列化");
    let back = PlaylistSet::from_json(&text);
    let playlist = back.tab(0).expect("第一页");

    assert_eq!(
        names(playlist),
        ["d", "d/a.mp4", "d/sub", "d/sub/b.mp4", "c.mp4"]
    );
    assert_eq!(depths(playlist), [0, 1, 1, 2, 0]);
    assert_eq!(playlist.index(), Some(1));
}

#[test]
fn 恢复历史顺序带回层级() {
    let mut playlist = tree_of(
        &[
            dir_item("d", 0),
            file_item("d/a.mp4", 1),
            file_item("b.mp4", 0),
        ],
        1,
    );

    assert!(playlist.move_to(2, DropSpot::Before(0)));
    assert_eq!(names(&playlist), ["b.mp4", "d", "d/a.mp4"]);
    assert_eq!(depths(&playlist), [0, 0, 1]);

    assert!(playlist.restore_version(0));
    assert_eq!(names(&playlist), ["d", "d/a.mp4", "b.mp4"]);
    assert_eq!(depths(&playlist), [0, 1, 0], "层级跟着旧版本一起回来");
    assert_eq!(playlist.index(), Some(1), "当前项仍然跟着自己的文件");
}

#[test]
fn 老存档里的平铺列表层级都是零() {
    let playlist = sample(&["a.mp4", "b.mp4"], 0);
    assert_eq!(depths(&playlist), [0, 0]);
    assert!(playlist.items().iter().all(|item| !item.is_dir()));
}
