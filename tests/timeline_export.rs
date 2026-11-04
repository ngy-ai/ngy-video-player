//! 导出链路的端到端验证：真调 ffmpeg 生成素材、导出、读回时长。
//!
//! 需要本机 ffmpeg，默认忽略；手工跑：
//! `cargo test --test timeline_export -- --ignored`

use std::path::PathBuf;

use ngy_video_player::export::{ExportEvent, ExportTask};
use ngy_video_player::timeline::{ClipItem, Project, Tick, plan_sequence};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ngy-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

fn second(n: f64) -> Tick {
    Tick::from_secs(n)
}

#[test]
#[ignore = "需要本机 ffmpeg"]
fn 导出带空档的两段序列() {
    let dir = scratch("export-e2e");
    let clip = dir.join("clip.mp4");

    // 2 秒彩条 + 1kHz 音调：够切两段、也够读回时长。
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=30:duration=2",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=1000:duration=2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(&clip)
        .status()
        .expect("跑 ffmpeg 生成素材");
    assert!(status.success(), "生成测试素材失败");

    let mut project = Project::new();
    project.sequence.width = 320;
    project.sequence.height = 240;
    let asset = project.add_asset(&clip);
    let video = project.sequence.tracks[0].id;
    let audio = project.sequence.tracks[1].id;

    // 视频：0-1 秒、3-4 秒，中间两秒空档应当补黑场。
    for start in [Tick::ZERO, second(3.0)] {
        let id = project.sequence.alloc_id();
        project
            .sequence
            .track_mut(video)
            .unwrap()
            .clips
            .push(ClipItem::new(id, asset, Tick::ZERO, second(1.0), start));
    }
    let id = project.sequence.alloc_id();
    project
        .sequence
        .track_mut(audio)
        .unwrap()
        .clips
        .push(ClipItem::new(
            id,
            asset,
            Tick::ZERO,
            second(1.0),
            Tick::ZERO,
        ));

    let plan = plan_sequence(&project).expect("生成计划");
    let output = dir.join("out.mp4");

    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let _task = ExportTask::start(plan, output.clone(), project.sequence.duration(), tx)
        .expect("启动 ffmpeg");

    // 事件流会在 `Finished` / `Failed` 之后结束，直接阻塞读到第一个终态即可。
    let outcome = loop {
        match futures::executor::block_on(futures::StreamExt::next(&mut rx)) {
            Some(ExportEvent::Progress(_)) => {}
            Some(other) => break other,
            None => panic!("任务线程提前退出"),
        }
    };

    match outcome {
        ExportEvent::Finished(path) => assert_eq!(path, output),
        other => panic!("导出失败：{other:?}"),
    }
    assert!(output.is_file(), "输出文件不存在");

    let probe = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-i"])
        .arg(&output)
        .output()
        .expect("读回输出");
    let text = String::from_utf8_lossy(&probe.stderr);
    assert!(
        text.contains("Duration: 00:00:04"),
        "总时长应当是 4 秒（1 + 黑场 2 + 1）：\n{text}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
