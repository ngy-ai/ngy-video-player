//! 偏好的单测：最近工程的去重 / 上限，以及各类字段的默认值与存盘往返。

use super::*;

#[test]
fn 最近工程去重且最新在前() {
    let mut list: Vec<PathBuf> = Vec::new();
    push_recent(&mut list, std::path::Path::new("/a.ngyproj"));
    push_recent(&mut list, std::path::Path::new("/b.ngyproj"));
    push_recent(&mut list, std::path::Path::new("/a.ngyproj"));

    assert_eq!(
        list,
        vec![PathBuf::from("/a.ngyproj"), PathBuf::from("/b.ngyproj")],
        "再开一次已有的工程只把它提到最前，不添新条"
    );
}

#[test]
fn 最近工程只留固定条数() {
    let mut list: Vec<PathBuf> = Vec::new();
    for index in 0..(RECENT_PROJECTS_LIMIT + 3) {
        push_recent(
            &mut list,
            std::path::Path::new(&format!("/{index}.ngyproj")),
        );
    }
    assert_eq!(list.len(), RECENT_PROJECTS_LIMIT);
    assert_eq!(
        list[0],
        PathBuf::from(format!("/{}.ngyproj", RECENT_PROJECTS_LIMIT + 2)),
        "最新的在最前"
    );
}
