//! 播放列表的确定性行为：目录展开、自然序、m3u 解析、循环推进。

use ngy_video_player::playlist::{
    Playlist, PlaylistSet, SortMode, collect_dir, collect_tree, parse_playlist,
};
use std::fs;
use std::path::{Path, PathBuf};

// 集成测试的根文件是 crate root，`mod` 默认只在 `tests/` 同层找文件；
// 显式写 path 才能按「根文件 + 同名目录」的布局组织子模块。
#[path = "playlist/tree.rs"]
mod tree;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ngy-vp-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("创建临时目录");
    dir
}

fn touch(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, b"x").expect("写测试文件");
    path
}

#[test]
fn 目录展开按自然序排列() {
    let dir = scratch("natural");
    touch(&dir, "第10集.mp4");
    touch(&dir, "第2集.mp4");
    touch(&dir, "第1集.mp4");

    let files: Vec<String> = collect_dir(&dir)
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();

    assert_eq!(files, ["第1集.mp4", "第2集.mp4", "第10集.mp4"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 目录展开只收媒体文件() {
    let dir = scratch("filter");
    touch(&dir, "a.mkv");
    touch(&dir, "b.txt");
    touch(&dir, "c.srt");
    touch(&dir, "d.flac");

    let files: Vec<String> = collect_dir(&dir)
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();

    assert_eq!(files, ["a.mkv", "d.flac"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn m3u_相对路径按列表文件所在目录解析() {
    let dir = scratch("m3u");
    touch(&dir, "one.mp4");
    touch(&dir, "two.mp4");
    let list = dir.join("play.m3u");
    fs::write(
        &list,
        "#EXTM3U\n#EXTINF:10,第一个\none.mp4\n\n# 注释\ntwo.mp4\nmissing.mp4\n",
    )
    .expect("写 m3u");

    let entries = parse_playlist(&list);
    let names: Vec<String> = entries
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();

    // 不存在的条目被跳过，注释与 EXTINF 行被忽略。
    assert_eq!(names, ["one.mp4", "two.mp4"]);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn pls_解析_FileN_条目() {
    let dir = scratch("pls");
    touch(&dir, "a.mp4");
    let list = dir.join("play.pls");
    fs::write(
        &list,
        "[playlist]\nFile1=a.mp4\nTitle1=hello\nNumberOfEntries=1\n",
    )
    .expect("写 pls");

    let entries = parse_playlist(&list);
    assert_eq!(entries.len(), 1);
    assert!(entries[0].ends_with("a.mp4"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 播放到末尾_顺序模式停下_循环模式回绕() {
    let mut playlist = Playlist::default();
    playlist.replace(
        vec![
            PathBuf::from("a.mp4"),
            PathBuf::from("b.mp4"),
            PathBuf::from("c.mp4"),
        ],
        2,
    );

    assert_eq!(playlist.next_index(false), None, "顺序模式最后一个之后停下");
    assert_eq!(playlist.next_index(true), Some(0), "循环模式回到第一个");
    assert_eq!(playlist.previous_index(false), Some(1));
    assert_eq!(playlist.previous_index(true), Some(1));

    playlist.set_index(0);
    assert_eq!(playlist.previous_index(false), None);
    assert_eq!(playlist.previous_index(true), Some(2));
}

fn sample(items: &[&str], start: usize) -> Playlist {
    let mut playlist = Playlist::default();
    playlist.replace(items.iter().map(PathBuf::from).collect(), start);
    playlist
}

fn names(playlist: &Playlist) -> Vec<String> {
    playlist
        .items()
        .iter()
        .map(|item| item.path().to_string_lossy().to_string())
        .collect()
}

#[test]
fn 批量删除_非当前项只挪索引() {
    let mut playlist = sample(&["a.mp4", "b.mp4", "c.mp4", "d.mp4"], 2);

    let removal = playlist.remove_many(&[0, 1]);
    assert_eq!(removal.removed, 2);
    assert!(!removal.current_removed, "没删到当前项就不该换歌");
    assert_eq!(removal.current, Some(0));
    assert_eq!(playlist.index(), Some(0));
    assert_eq!(playlist.current().unwrap().to_string_lossy(), "c.mp4");
    assert_eq!(names(&playlist), ["c.mp4", "d.mp4"]);
}

#[test]
fn 批量删除_当前项落到后面第一个幸存者() {
    let mut playlist = sample(&["a.mp4", "b.mp4", "c.mp4", "d.mp4"], 1);

    let removal = playlist.remove_many(&[1, 2]);
    assert!(removal.current_removed);
    assert_eq!(removal.current, Some(1));
    assert_eq!(
        playlist.current().unwrap().to_string_lossy(),
        "d.mp4",
        "当前项和它后面的一起删掉时，应该接上后面还活着的那一项"
    );
}

#[test]
fn 删除末尾当前项时回退到前一个() {
    let mut playlist = sample(&["a.mp4", "b.mp4", "c.mp4"], 2);

    let removal = playlist.remove_many(&[2]);
    assert!(removal.current_removed);
    assert_eq!(removal.current, Some(1));
    assert_eq!(playlist.current().unwrap().to_string_lossy(), "b.mp4");
}

#[test]
fn 越界与重复索引被忽略() {
    let mut playlist = sample(&["a.mp4", "b.mp4", "c.mp4"], 0);

    let removal = playlist.remove_many(&[1, 1, 99]);
    assert_eq!(removal.removed, 1);
    assert_eq!(names(&playlist), ["a.mp4", "c.mp4"]);
    assert_eq!(playlist.index(), Some(0), "删掉后面的项不影响当前项");
}

#[test]
fn 删光之后列表为空且没有当前项() {
    let mut playlist = sample(&["a.mp4", "b.mp4"], 1);

    let removal = playlist.remove_many(&[0, 1]);
    assert_eq!(removal.removed, 2);
    assert!(removal.current_removed);
    assert_eq!(removal.current, None);
    assert!(playlist.is_empty());
    assert_eq!(playlist.index(), None);
    assert_eq!(playlist.current(), None);
}

#[test]
fn 清空列表不留下当前项() {
    let mut playlist = sample(&["a.mp4", "b.mp4"], 1);

    playlist.clear();
    assert!(playlist.is_empty());
    assert_eq!(playlist.len(), 0);
    assert_eq!(playlist.index(), None);
    assert_eq!(playlist.current(), None);
    assert_eq!(playlist.next_index(true), None);
    assert_eq!(playlist.previous_index(true), None);
}

#[test]
fn 多个列表各有各的内容和位置() {
    let mut set = PlaylistSet::default();
    assert_eq!(set.len(), 1, "面板至少有一页落脚");
    set.active_playlist_mut()
        .replace(vec![PathBuf::from("a.mp4"), PathBuf::from("b.mp4")], 0);

    let second = set.add("第二份");
    assert_eq!(second, 1);
    assert_eq!(set.active(), 1, "新建之后切到新页");
    assert!(set.active_playlist().is_empty());

    set.active_playlist_mut()
        .replace(vec![PathBuf::from("c.mp4")], 0);
    set.set_active(0);
    assert_eq!(set.active_playlist().len(), 2);
    assert_eq!(set.active_playlist().index(), Some(0), "切页不丢页内位置");
    assert_eq!(set.tab(0).unwrap().len(), 2);
}

#[test]
fn 关闭列表后页码仍然有效() {
    let mut set = PlaylistSet::default();
    set.add("B");
    set.add("C");
    assert_eq!(set.active(), 2);

    assert!(set.remove(1), "关掉中间的 B");
    assert_eq!(set.len(), 2);
    assert_eq!(set.active(), 1, "当前页跟着左移，仍指向 C");
    assert_eq!(set.tabs()[1].name, "C");
}

#[test]
fn 只剩一页时不允许关闭() {
    let mut set = PlaylistSet::default();
    assert!(!set.remove(0));
    assert_eq!(set.len(), 1);
    assert!(!set.remove(5), "越界下标也不动状态");
}

#[test]
fn 第一页是默认列表且改不动删不掉() {
    let mut set = PlaylistSet::default();
    assert_eq!(set.len(), 1);
    assert_eq!(set.tabs()[0].name, "默认");
    assert!(set.is_default(0));

    assert!(!set.rename(0, "我的列表"), "默认列表不许改名");
    assert_eq!(set.tabs()[0].name, "默认");

    set.add("第二页");
    set.set_active(0);
    assert!(!set.remove(0), "默认列表不许删除");
    assert_eq!(set.len(), 2);
    assert!(!set.rename_active("还是一样"), "默认页改名请求也拒绝");
    assert_eq!(set.tabs()[0].name, "默认");

    // 别的页可以随便改，但「默认」是保留名：两页同名的话用户分不清落点。
    assert!(set.rename(1, "剧集"));
    assert_eq!(set.tabs()[1].name, "剧集");
    assert!(!set.rename(1, "默认"));
    assert_eq!(set.tabs()[1].name, "剧集");
    assert!(!set.rename(9, "不存在"), "越界索引返回失败而不是 panic");
}

#[test]
fn 存档往返保留内容与页码() {
    let mut set = PlaylistSet::default();
    set.active_playlist_mut()
        .replace(vec![PathBuf::from("a.mp4"), PathBuf::from("b.mp4")], 1);
    set.add("电影");

    let text = serde_json::to_string(&set).expect("序列化播放列表");
    let back = PlaylistSet::from_json(&text);

    assert_eq!(back.len(), 2);
    assert_eq!(back.active(), 1);
    assert_eq!(back.tabs()[0].name, "默认");
    assert_eq!(back.tabs()[0].playlist.items().len(), 2);
    assert_eq!(back.tabs()[0].playlist.index(), Some(1));
    assert_eq!(back.tabs()[1].name, "电影");
}

#[test]
fn 老存档的第一页变成默认列表() {
    // 升级前第一页可能是用户自己起的名；这一版它是固定的「默认」，
    // 内容照旧保留，不额外插一页把用户的东西挪走。
    let text = r#"{"tabs":[{"name":"剧集","playlist":{"items":["a.mp4"]}}],"active":0}"#;
    let set = PlaylistSet::from_json(text);

    assert_eq!(set.len(), 1);
    assert_eq!(set.tabs()[0].name, "默认");
    assert_eq!(set.tabs()[0].playlist.items().len(), 1);
}

#[test]
fn 存档里的越界索引与空名字被校正() {
    let text = r#"{"tabs":[{"name":"默认","playlist":{}},
        {"name":"  ","playlist":{"items":["a.mp4"],"index":7}}],"active":9}"#;
    let set = PlaylistSet::from_json(text);

    assert_eq!(set.len(), 2);
    assert_eq!(set.active(), 1);
    assert_eq!(set.tabs()[0].name, "默认");
    assert_eq!(set.tabs()[1].name, "列表 2");
    assert_eq!(set.tabs()[1].playlist.index(), None, "越界索引应被丢掉");
}

#[test]
fn 坏存档退回默认列表() {
    let set = PlaylistSet::from_json("这不是 json");
    assert_eq!(set.len(), 1);
    assert_eq!(set.tabs()[0].name, "默认");
    assert!(set.active_playlist().is_empty());
}

#[test]
fn 删除当前项后索引落回有效位置() {
    let mut playlist = Playlist::default();
    playlist.replace(
        vec![
            PathBuf::from("a.mp4"),
            PathBuf::from("b.mp4"),
            PathBuf::from("c.mp4"),
        ],
        1,
    );

    playlist.remove(1);
    assert_eq!(playlist.len(), 2);
    assert_eq!(playlist.index(), Some(1));
    assert_eq!(
        playlist.current().unwrap().to_string_lossy(),
        "c.mp4",
        "删掉当前项后应落到它的下一个"
    );

    playlist.remove(1);
    assert_eq!(playlist.index(), Some(0));

    playlist.remove(0);
    assert_eq!(playlist.index(), None);
    assert!(playlist.is_empty());
}

#[test]
fn 追加文件跳过重复项() {
    let dir = scratch("append");
    let first = touch(&dir, "a.mp4");
    let second = touch(&dir, "b.mp4");
    let mut playlist = Playlist::default();

    playlist.append_paths(&[first.clone(), second.clone(), first.clone()]);
    assert_eq!(
        names(&playlist),
        [
            first.to_string_lossy().to_string(),
            second.to_string_lossy().to_string()
        ]
    );

    // 整个目录再展开一次也不会变成两份。
    playlist.append_paths(std::slice::from_ref(&dir));
    assert_eq!(playlist.len(), 2);

    // 追加不动当前项：队首还是原来那一项。
    assert_eq!(playlist.index(), None, "空列表里追加不会凭空多出当前项");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 追加展开好的条目按路径去重() {
    let dir = scratch("append-entries");
    touch(&dir, "a.mp4");
    let entries = collect_tree(&dir);
    assert_eq!(entries.len(), 2, "文件夹一行 + 里面一个文件");

    let mut playlist = Playlist::default();
    assert_eq!(playlist.append_entries(entries.clone()), 1);
    assert_eq!(playlist.len(), 2);

    // 同一棵树再并一次：文件夹连同子树整段跳过，不会变成两份。
    assert_eq!(playlist.append_entries(entries), 0);
    assert_eq!(playlist.len(), 2);
    assert_eq!(playlist.index(), None, "追加不会凭空多出当前项");
    let _ = fs::remove_dir_all(&dir);
}

// -- 排序 ----------------------------------------------------------------

fn file(dir: &Path, name: &str) -> PathBuf {
    dir.join(name)
}

#[test]
fn 按名称排序用自然序() {
    let mut playlist = sample(&["第10集.mp4", "第2集.mp4", "第1集.mp4"], 0);

    assert!(playlist.sort_items(SortMode::Name));
    assert_eq!(names(&playlist), ["第1集.mp4", "第2集.mp4", "第10集.mp4"]);
    assert_eq!(playlist.sort_mode(), SortMode::Name);
}

#[test]
fn 排序后当前项跟着文件走() {
    let mut playlist = sample(&["b.mp4", "a.mp4", "c.mp4"], 0);
    assert_eq!(playlist.current().unwrap().to_string_lossy(), "b.mp4");

    playlist.sort_items(SortMode::Name);
    assert_eq!(names(&playlist), ["a.mp4", "b.mp4", "c.mp4"]);
    assert_eq!(
        playlist.index(),
        Some(1),
        "当前项是跟着文件换位置的，不是钉在原来那个下标上"
    );
    assert_eq!(playlist.current().unwrap().to_string_lossy(), "b.mp4");
}

#[test]
fn 顺序没变时不占历史() {
    let mut playlist = sample(&["a.mp4", "b.mp4"], 0);

    assert!(!playlist.sort_items(SortMode::Name), "本来就是有序的");
    assert!(playlist.history().is_empty());
    assert_eq!(
        playlist.sort_mode(),
        SortMode::Name,
        "排法仍然记下来给菜单打勾"
    );

    // 再点一次同样的排序也不该多出一版。
    assert!(!playlist.sort_items(SortMode::Name));
    assert!(playlist.history().is_empty());
}

#[test]
fn 按修改时间排序_读不到时间的排最后() {
    let dir = scratch("mtime");
    let older = file(&dir, "old.mp4");
    let newer = file(&dir, "new.mp4");
    touch(&dir, "old.mp4");
    // 文件系统的时间戳精度至少到秒，睡够一秒才能保证两者的修改时间不同。
    std::thread::sleep(std::time::Duration::from_millis(1100));
    touch(&dir, "new.mp4");
    let gone = file(&dir, "gone.mp4");

    let mut playlist = Playlist::default();
    playlist.replace(vec![newer.clone(), gone, older.clone()], 0);
    playlist.sort_items(SortMode::Modified);

    assert_eq!(
        names(&playlist),
        [
            older.to_string_lossy().to_string(),
            newer.to_string_lossy().to_string(),
            file(&dir, "gone.mp4").to_string_lossy().to_string()
        ],
        "旧的在前面；读不到修改时间的排在最后"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 按添加时间排序回到加入顺序() {
    // 加入时间只从存档里来（同一秒内加进来的几项会拿到同一个值），
    // 这里直接给一份「加入顺序」和「当前排列」不一样的列表。
    let text = r#"{"tabs":[{"name":"t","playlist":{"items":[
        {"path":"c.mp4","added_at":30},
        {"path":"a.mp4","added_at":10},
        {"path":"b.mp4","added_at":20}
    ],"index":0}}],"active":0}"#;
    let set = PlaylistSet::from_json(text);
    let mut playlist = set.tab(0).cloned().expect("第一页");

    assert!(playlist.sort_items(SortMode::Added));
    assert_eq!(names(&playlist), ["a.mp4", "b.mp4", "c.mp4"]);
    assert_eq!(playlist.index(), Some(2), "当前项仍然是 c.mp4");
}

#[test]
fn 手工拖动重排并记下旧顺序() {
    let mut playlist = sample(&["a.mp4", "b.mp4", "c.mp4"], 0);

    assert!(playlist.move_item(0, 3), "把第一项拖到末尾");
    assert_eq!(names(&playlist), ["b.mp4", "c.mp4", "a.mp4"]);
    assert_eq!(playlist.sort_mode(), SortMode::Manual);
    assert_eq!(playlist.index(), Some(2), "当前项跟着 a.mp4 走");
    assert_eq!(playlist.history().len(), 1);
    assert_eq!(playlist.history()[0].mode(), SortMode::Added);

    // 插回自己所在的位置等于没动，不该再占一格历史。
    assert!(!playlist.move_item(1, 1));
    assert!(!playlist.move_item(1, 2));
    assert_eq!(playlist.history().len(), 1);
}

#[test]
fn 历史顺序最多留三版() {
    let mut playlist = sample(&["c.mp4", "b.mp4", "a.mp4"], 0);

    playlist.sort_items(SortMode::Name); // [a b c]
    playlist.move_item(0, 3); // [b c a]
    playlist.sort_items(SortMode::Added); // 全是同一秒加的，顺序不变
    playlist.move_item(0, 2); // [c b a]
    playlist.sort_items(SortMode::Name); // [a b c]

    assert_eq!(playlist.history().len(), 3, "只留最近三版");
    assert_eq!(playlist.history()[0].mode(), SortMode::Manual);
}

#[test]
fn 恢复历史顺序() {
    let mut playlist = sample(&["c.mp4", "a.mp4", "b.mp4"], 0);

    playlist.sort_items(SortMode::Name);
    assert_eq!(names(&playlist), ["a.mp4", "b.mp4", "c.mp4"]);

    assert!(playlist.restore_version(0));
    assert_eq!(names(&playlist), ["c.mp4", "a.mp4", "b.mp4"]);
    assert_eq!(playlist.sort_mode(), SortMode::Added, "恢复的排法一起回来");
    assert_eq!(
        playlist.current().unwrap().to_string_lossy(),
        "c.mp4",
        "当前项仍然跟着自己的文件"
    );
    assert_eq!(
        playlist.history()[0].mode(),
        SortMode::Name,
        "还原本身也进历史，还能再撤销回去"
    );
}

#[test]
fn 存档往返保留排序方式与历史() {
    let mut set = PlaylistSet::default();
    let playlist = set.active_playlist_mut();
    playlist.replace(
        vec![
            PathBuf::from("c.mp4"),
            PathBuf::from("a.mp4"),
            PathBuf::from("b.mp4"),
        ],
        0,
    );
    playlist.sort_items(SortMode::Name);

    let text = serde_json::to_string(&set).expect("序列化播放列表");
    let back = PlaylistSet::from_json(&text);
    let playlist = back.tab(0).unwrap();

    assert_eq!(names(playlist), ["a.mp4", "b.mp4", "c.mp4"]);
    assert_eq!(playlist.sort_mode(), SortMode::Name);
    assert_eq!(playlist.history().len(), 1);
    assert_eq!(playlist.history()[0].len(), 3);
}

#[test]
fn 老存档里的裸路径仍然能读() {
    let text =
        r#"{"tabs":[{"name":"旧","playlist":{"items":["a.mp4","b.mp4"],"index":1}}],"active":0}"#;
    let set = PlaylistSet::from_json(text);
    let playlist = set.tab(0).unwrap();

    assert_eq!(names(playlist), ["a.mp4", "b.mp4"]);
    assert_eq!(playlist.index(), Some(1));
    assert_eq!(playlist.sort_mode(), SortMode::Added);
    assert!(playlist.history().is_empty());
    assert_eq!(
        playlist.items()[0].added_at(),
        0,
        "老存档没有加入时间，按添加时间排序时就保持原顺序"
    );
}

#[test]
fn 列表页可以重命名() {
    let mut set = PlaylistSet::default();
    set.add("第二页");
    assert_eq!(set.active(), 1);

    assert!(set.rename(1, "剧集"));
    assert_eq!(set.tabs()[1].name, "剧集");

    assert!(set.rename_active("综艺"));
    assert_eq!(set.tabs()[1].name, "综艺");
    assert_eq!(set.tabs()[0].name, "默认", "只改当前页，默认页不动");

    // 越界索引静默忽略：右键菜单弹出后页签可能刚好被关掉了。
    assert!(!set.rename(9, "不存在"));
    assert_eq!(set.len(), 2);
}

#[test]
fn 新页默认名字跳过已占用的号() {
    let mut set = PlaylistSet::default();
    assert_eq!(set.auto_name(), "列表 1", "默认页不占「列表 N」的号");

    let first = set.auto_name();
    set.add(first);
    assert_eq!(set.auto_name(), "列表 2");

    let second = set.auto_name();
    set.add(second);
    assert!(set.rename(1, "剧集"));
    assert_eq!(set.auto_name(), "列表 1", "改名腾出来的号可以再用");
}

#[test]
fn 页签可以新建和关闭且至少留一页() {
    let mut set = PlaylistSet::default();
    set.add("第二页");
    set.add("第三页");
    assert_eq!(set.len(), 3);

    assert!(set.remove(2));
    assert_eq!(set.active(), 1);
    assert!(set.remove(1));
    assert_eq!(set.len(), 1);
    assert!(!set.remove(0), "默认页是最后一页，不让关");
    assert_eq!(set.len(), 1);
}
