//! 自动保存：计时、落盘、待恢复判断与拒绝后的行为。

use super::*;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ngy-vp-autosave-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("创建临时目录");
    dir
}

fn secs(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

#[test]
fn 满三十秒没有新操作才写() {
    let mut timer = AutoSave::new();
    assert!(!timer.should_write(secs(0)), "没有编辑就不写");

    timer.mark_dirty(secs(0));
    assert!(!timer.should_write(secs(29)));
    assert!(timer.should_write(secs(30)));
    assert!(!timer.should_write(secs(31)), "写完清掉计时");
    assert!(!timer.is_dirty());
}

#[test]
fn 持续编辑会不断重新计时() {
    let mut timer = AutoSave::new();
    timer.mark_dirty(secs(0));
    timer.mark_dirty(secs(20));

    assert!(!timer.should_write(secs(40)), "20 秒时又改了一次");
    assert!(timer.should_write(secs(50)));
}

#[test]
fn 正常保存之后不再自动保存() {
    let mut timer = AutoSave::new();
    timer.mark_dirty(secs(0));
    timer.mark_saved();

    assert!(!timer.should_write(secs(100)));
    assert!(!timer.is_dirty());
}

#[test]
fn 间隔可以自定义快进() {
    let mut timer = AutoSave::with_interval(secs(1));
    timer.mark_dirty(Duration::ZERO);
    assert!(timer.should_write(secs(1)));
}

#[test]
fn 自动保存能被打开且不留备份() {
    let dir = scratch("write");
    let project_path = dir.join("p.ngyproj");
    let mut project = Project::new();
    project.sequence.name = "崩溃前".to_string();

    let path = write(&project, &project_path).expect("写自动保存");
    assert_eq!(path, autosave_path(&project_path));

    let back = Project::open(&path).expect("自动保存也是合法工程");
    assert_eq!(back.sequence.name, "崩溃前");
    assert!(
        !with_suffix(&path, ".bak").exists(),
        "自动保存不该每 30 秒多一份 .bak"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 更晚的自动保存才提示恢复() {
    let dir = scratch("pending");
    let project_path = dir.join("p.ngyproj");
    let mut project = Project::new();
    project.save(&project_path).expect("保存工程");

    // 文件系统的时间戳精度可能只到秒，睡够一秒才能保证「更晚」。
    std::thread::sleep(Duration::from_millis(1100));
    project.sequence.name = "崩溃前".to_string();
    let autosave = write(&project, &project_path).expect("写自动保存");
    assert_eq!(pending(&project_path), Some(autosave.clone()));

    let rejected = dismiss(&project_path).expect("拒绝恢复");
    assert!(!autosave.exists(), "已经改名为拒绝记录");
    assert!(rejected.exists(), "内容留着，用户可以手动打开");
    assert_eq!(pending(&project_path), None, "同一份不再提示");

    // 紧接着正常保存一次：自动保存比工程文件旧，不再提示。
    write(&project, &project_path).expect("再写一份自动保存");
    project.save(&project_path).expect("正常保存");
    assert_eq!(pending(&project_path), None);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 工程文件丢了也能从自动保存恢复() {
    let dir = scratch("lost");
    let project_path = dir.join("p.ngyproj");
    let project = Project::new();

    let autosave = write(&project, &project_path).expect("写自动保存");
    assert_eq!(pending(&project_path), Some(autosave));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn 正常退出清掉自动保存() {
    let dir = scratch("clear");
    let project_path = dir.join("p.ngyproj");
    let autosave = write(&Project::new(), &project_path).expect("写自动保存");
    assert!(autosave.exists());

    clear(&project_path);
    assert!(!autosave.exists());
    assert_eq!(pending(&project_path), None);

    let _ = fs::remove_dir_all(&dir);
}
