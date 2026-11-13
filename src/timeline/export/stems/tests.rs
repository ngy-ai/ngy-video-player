//! 分轨导出计划的形状：一轨一个文件、命名安全、滤镜只留音频。

use super::*;
use crate::timeline::{AssetId, ClipItem, Id, Project, Rational, Tick};

fn empty_project() -> Project {
    Project::new_with(crate::timeline::SequenceSpec::new(
        1920,
        1080,
        Rational::FPS_25,
    ))
}

/// 往 `track` 上放一个 2 秒片段（素材内 0–2 秒，时间线 0 起）。
fn put(project: &mut Project, track: Id, asset: AssetId) {
    let sequence = project.sequence_mut();
    let index = sequence
        .tracks
        .iter()
        .position(|candidate| candidate.id == track)
        .expect("轨道在序列里");
    let id = sequence.alloc_id();
    sequence.tracks[index].clips.push(ClipItem::new(
        id,
        asset,
        Tick::ZERO,
        Tick::from_secs(2.0),
        Tick::ZERO,
    ));
}

#[test]
fn 每条听得见的音频轨一条分轨() {
    let dir = temp_dir_suffix("stems");
    let mut project = empty_project();
    let first = project.sequence().tracks[1].id;
    let second = project.sequence_mut().add_track(TrackKind::Audio);
    let asset = project.add_asset(&sample_file(&dir));
    put(&mut project, first, asset);
    put(&mut project, second, asset);

    let stems = plan_stems(&project, &dir).expect("能导出分轨");
    assert_eq!(stems.len(), 2);
    assert!(
        stems[0].path.ends_with("01-音频 1.wav"),
        "{:?}",
        stems[0].path
    );
    assert!(
        stems[1].path.ends_with("02-音频 2.wav"),
        "{:?}",
        stems[1].path
    );
    assert_eq!(stems[0].plan.audio_label.as_deref(), Some("[outa]"));
    assert!(stems[0].plan.video_label.is_none(), "分轨里不该有画面");
    assert!(stems[0].plan.output_args.iter().any(|arg| arg == "-vn"));
    // 链里要有 concat，且标签是 [outa]。
    assert!(stems[0].plan.filter.contains("concat=n=1:v=0:a=1[outa]"));
    assert_eq!(stems[0].plan.inputs.len(), 1, "同一个素材只加一次 -i");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 静音与空轨不出分轨() {
    let dir = temp_dir_suffix("stems-skip");
    let mut project = empty_project();
    let silent = project.sequence().tracks[1].id;
    let _empty = project.sequence_mut().add_track(TrackKind::Audio);
    let asset = project.add_asset(&sample_file(&dir));
    put(&mut project, silent, asset);
    project.sequence_mut().track_mut(silent).unwrap().muted = true;

    let error = plan_stems(&project, &dir).expect_err("没有可导出的轨");
    assert_eq!(error, ExportError::NoAudioTrack);
    assert!(error.to_string().contains("音频轨"));

    // 一条片段都没有的序列直接是「时间线空的」。
    let blank = empty_project();
    assert_eq!(plan_stems(&blank, &dir), Err(ExportError::Empty));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 独奏只导独奏的那条() {
    let dir = temp_dir_suffix("stems-solo");
    let mut project = empty_project();
    let first = project.sequence().tracks[1].id;
    let second = project.sequence_mut().add_track(TrackKind::Audio);
    let asset = project.add_asset(&sample_file(&dir));
    put(&mut project, first, asset);
    put(&mut project, second, asset);
    project.sequence_mut().track_mut(second).unwrap().solo = true;

    let stems = plan_stems(&project, &dir).expect("能导出分轨");
    assert_eq!(stems.len(), 1);
    assert_eq!(stems[0].track, second.raw());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 轨道名里的危险字符被换掉() {
    assert_eq!(sanitize("音乐/主轨", 1), "音乐_主轨");
    assert_eq!(sanitize("  ..  ", 3), "音轨3", "只剩点和空格就是空名");
    assert_eq!(sanitize("", 2), "音轨2");
    assert_eq!(sanitize("C:\\temp:1", 1), "C__temp_1");
    // 太长截断，别把路径顶到系统上限。
    let long = "长".repeat(80);
    assert_eq!(sanitize(&long, 1).chars().count(), 60);
}

/// 一个真实存在的素材文件：`plan_stems` 会检查素材还在不在，只有路径不够。
fn sample_file(dir: &Path) -> PathBuf {
    let path = dir.join("a.wav");
    let _ = std::fs::write(&path, b"");
    path
}

/// 测试用的临时目录：不碰工程、不落在仓库里。
fn temp_dir_suffix(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ngy-stems-{}-{name}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}
