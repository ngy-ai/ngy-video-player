//! 播放列表的确定性行为：目录展开、自然序、m3u 解析、循环推进。

use ngy_video_player::playlist::{Playlist, collect_dir, parse_playlist};
use std::fs;
use std::path::{Path, PathBuf};

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
