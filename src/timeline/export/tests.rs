//! 导出计划的字符串形状：片段裁剪、空档补黑、输入去重、缺素材。

use std::path::{Path, PathBuf};

use super::*;
use crate::timeline::{AssetId, ClipItem, Id, Project, Tick};

fn s(n: i64) -> Tick {
    Tick::from_micros(n * 1_000_000)
}

/// 临时目录：`add_asset` 会判断文件在不在，所以测试得真的造一个文件。
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ngy-export-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        Self { dir }
    }

    /// 写一个 1 字节的假素材：登记只看文件在不在，不解码。
    fn media(&self, name: &str) -> PathBuf {
        let path = self.dir.join(name);
        std::fs::create_dir_all(path.parent().expect("有父目录")).expect("建素材目录");
        std::fs::write(&path, b"x").expect("写素材");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// 建一个只有一条视频轨的工程，片段都引用同一个素材。
fn project_with(scratch: &Scratch, clips: &[(Tick, Tick, Tick)]) -> (Project, Vec<Id>) {
    let mut project = Project::new();
    let asset = project.add_asset(&scratch.media("media/a.mp4"));
    let track = project.sequence.tracks[0].id;
    let mut ids = Vec::new();

    for (source_in, source_out, start) in clips {
        let id = project.sequence.alloc_id();
        let lane = project.sequence.track_mut(track).expect("视频轨存在");
        lane.clips
            .push(ClipItem::new(id, asset, *source_in, *source_out, *start));
        lane.sort_clips();
        ids.push(id);
    }

    (project, ids)
}

#[test]
fn 空时间线拒绝导出() {
    let project = Project::new();
    assert_eq!(plan_sequence(&project).unwrap_err(), ExportError::Empty);
}

#[test]
fn 视频轨全被隐藏时拒绝导出() {
    let scratch = Scratch::new("muted");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    project.sequence.tracks[0].muted = true;
    assert_eq!(
        plan_sequence(&project).unwrap_err(),
        ExportError::NoVideoTrack
    );
}

#[test]
fn 单片段导出包含裁剪与拼接() {
    let scratch = Scratch::new("single");
    let (project, _) = project_with(&scratch, &[(s(1), s(3), Tick::ZERO)]);
    let plan = plan_sequence(&project).expect("能导出");

    assert_eq!(plan.inputs.len(), 1);
    assert!(
        plan.filter.contains("trim=start=1.000000:end=3.000000"),
        "{}",
        plan.filter
    );
    assert!(
        plan.filter.contains("concat=n=1:v=1:a=0[outv]"),
        "{}",
        plan.filter
    );
    assert_eq!(plan.video_label.as_deref(), Some("[outv]"));
    assert!(plan.audio_label.is_none(), "没有音频轨");
    assert_eq!(plan.inputs[0].file_name().unwrap(), "a.mp4");
}

#[test]
fn 同一素材的两个片段只算一个输入() {
    let scratch = Scratch::new("dedupe");
    let (project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO), (s(0), s(2), s(2))]);
    let plan = plan_sequence(&project).expect("能导出");

    assert_eq!(plan.inputs.len(), 1, "同一个文件只 -i 一次");
    assert!(plan.filter.contains("concat=n=2"), "{}", plan.filter);
    assert!(
        plan.filter.matches("[0:v]").count() == 2,
        "两个片段都指向输入 0：{}",
        plan.filter
    );
}

#[test]
fn 片段之间的空档补黑场() {
    let scratch = Scratch::new("gap");
    let (project, _) = project_with(&scratch, &[(s(0), s(1), Tick::ZERO), (s(0), s(1), s(3))]);
    let plan = plan_sequence(&project).expect("能导出");

    assert!(plan.filter.contains("color=c=black"), "{}", plan.filter);
    assert!(
        plan.filter.contains("d=2.000000"),
        "空档是 3 - 1 = 2 秒：{}",
        plan.filter
    );
    assert!(
        plan.filter.contains("concat=n=3"),
        "两段画面 + 一段黑场：{}",
        plan.filter
    );
}

#[test]
fn 音频轨参与导出并统一到四十八千赫() {
    let scratch = Scratch::new("audio");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    let asset = project.assets[0].id;
    let audio = project.sequence.tracks[1].id;
    let id = project.sequence.alloc_id();
    project
        .sequence
        .track_mut(audio)
        .expect("音频轨存在")
        .clips
        .push(ClipItem::new(id, asset, s(0), s(2), Tick::ZERO));

    let plan = plan_sequence(&project).expect("能导出");
    assert!(plan.filter.contains("aresample=48000"), "{}", plan.filter);
    assert!(
        plan.filter.contains("concat=n=1:v=0:a=1[outa]"),
        "{}",
        plan.filter
    );
    assert_eq!(plan.audio_label.as_deref(), Some("[outa]"));
    assert!(
        plan.output_args.contains(&"aac".to_string()),
        "有音频就要编码音频"
    );
}

#[test]
fn 没有音频时显式关掉音频() {
    let scratch = Scratch::new("silent");
    let (project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    let plan = plan_sequence(&project).expect("能导出");
    assert!(plan.output_args.contains(&"-an".to_string()));
    assert!(!plan.output_args.contains(&"aac".to_string()));
}

#[test]
fn 素材缺失在生成计划时就报错() {
    let scratch = Scratch::new("missing");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    project.assets[0].missing = true;

    match plan_sequence(&project).unwrap_err() {
        ExportError::MissingAsset(name) => assert_eq!(name, "a.mp4"),
        other => panic!("应当是缺素材，实际是 {other:?}"),
    }
}

#[test]
fn 片段引用了不存在的素材时报错() {
    let mut project = Project::new();
    let track = project.sequence.tracks[0].id;
    let id = project.sequence.alloc_id();
    project
        .sequence
        .track_mut(track)
        .unwrap()
        .clips
        .push(ClipItem::new(id, AssetId::new(99), s(0), s(1), Tick::ZERO));

    assert_eq!(
        plan_sequence(&project).unwrap_err(),
        ExportError::UnknownAsset(99)
    );
}

#[test]
fn 参数拼装把输入滤镜与映射按顺序排好() {
    let scratch = Scratch::new("args");
    let (project, _) = project_with(&scratch, &[(s(0), s(1), Tick::ZERO)]);
    let plan = plan_sequence(&project).expect("能导出");
    let args = plan.args();

    assert_eq!(args[0], "-i");
    assert!(args.contains(&"-filter_complex".to_string()));
    assert!(args.contains(&"[outv]".to_string()));
    assert!(args.contains(&"-crf".to_string()));
    // 输出文件由调用方追加：参数里只该有一个 .mp4（那个输入素材）。
    assert_eq!(
        args.iter().filter(|arg| arg.ends_with(".mp4")).count(),
        1,
        "输出文件不该出现在计划里"
    );
}

#[test]
fn 素材路径是绝对路径() {
    let scratch = Scratch::new("absolute");
    let (project, _) = project_with(&scratch, &[(s(0), s(1), Tick::ZERO)]);
    let plan = plan_sequence(&project).expect("能导出");
    assert!(
        Path::new(&plan.inputs[0]).is_absolute(),
        "ffmpeg 子进程的工作目录不一定和工程一致"
    );
}
