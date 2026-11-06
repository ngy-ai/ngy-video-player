//! 导出链路的端到端验证：真调 ffmpeg 生成素材、导出、读回时长。
//!
//! 需要本机 ffmpeg，默认忽略；手工跑：
//! `cargo test --test timeline_export -- --ignored`
//!
//! 纯字符串单测只能钉住滤镜的「形状」，钉不住 ffmpeg 的连线规则（pad 类型、
//! 标签是否都存在、concat 两端格式是否一致）。这两条用例负责把整张图真跑一遍。

use std::path::{Path, PathBuf};
use std::process::Command;

use ngy_video_player::export::{ExportEvent, ExportTask};
use ngy_video_player::timeline::{
    AssetId, ClipItem, Id, Project, Rational, SequenceSpec, Tick, TrackKind, Transition,
    TransitionKind, plan_sequence,
};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ngy-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");
    dir
}

fn second(n: f64) -> Tick {
    Tick::from_secs(n)
}

/// 2 秒彩条 + 1kHz 音调：够切好几个片段，也够读回时长。
fn make_clip(dir: &Path) -> PathBuf {
    let clip = dir.join("clip.mp4");
    let status = Command::new("ffmpeg")
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
    clip
}

/// 往指定轨放一个片段，返回片段 id。
fn put(
    project: &mut Project,
    track: Id,
    asset: AssetId,
    source_in: f64,
    source_out: f64,
    start: f64,
) -> Id {
    let id = project.sequence.alloc_id();
    project
        .sequence
        .track_mut(track)
        .unwrap()
        .clips
        .push(ClipItem::new(
            id,
            asset,
            second(source_in),
            second(source_out),
            second(start),
        ));
    id
}

/// 跑完一条导出任务；失败时把 ffmpeg 的最后几行带出来。
fn run_export(dir: &Path, project: &Project, name: &str) -> PathBuf {
    let plan = plan_sequence(project).expect("生成计划");
    let output = dir.join(name);

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
    output
}

/// `ffmpeg -i` 的 stderr 里有 Duration 行，用它读回成片时长。
fn assert_duration(output: &Path, expected: &str) {
    let probe = Command::new("ffmpeg")
        .args(["-hide_banner", "-i"])
        .arg(output)
        .output()
        .expect("读回输出");
    let text = String::from_utf8_lossy(&probe.stderr);
    let needle = format!("Duration: {expected}");
    assert!(
        text.contains(needle.as_str()),
        "总时长应当是 {expected}：\n{text}"
    );
}

/// `ffmpeg -i` 的 stderr 里有 `..., 30 fps, ...`，用它读回成片帧率。
fn assert_frame_rate(output: &Path, expected: &str) {
    let probe = Command::new("ffmpeg")
        .args(["-hide_banner", "-i"])
        .arg(output)
        .output()
        .expect("读回输出");
    let text = String::from_utf8_lossy(&probe.stderr);
    let needle = format!(", {expected} fps");
    assert!(
        text.contains(needle.as_str()),
        "帧率应当是 {expected} fps：\n{text}"
    );
}

/// M9-T03：25fps 建的工程改成 30fps 之后导出，帧率跟新规格走、总时长不变。
#[test]
#[ignore = "需要本机 ffmpeg"]
fn 改帧率之后导出用新规格() {
    let dir = scratch("export-spec");
    let clip = make_clip(&dir);

    let mut project = Project::new_with(SequenceSpec::new(320, 240, Rational::FPS_25));
    let asset = project.add_asset(&clip);
    let video = project.sequence.tracks[0].id;
    let audio = project.sequence.tracks[1].id;
    put(&mut project, video, asset, 0.0, 2.0, 0.0);
    put(&mut project, audio, asset, 0.0, 2.0, 0.0);
    assert_eq!(project.sequence.fps, Rational::FPS_25);

    // 只换帧率：片段的时间一个都没动，成片还是两秒。
    project.sequence.fps = Rational::FPS_30;

    let output = run_export(&dir, &project, "out30.mp4");
    assert_duration(&output, "00:00:02");
    assert_frame_rate(&output, "30");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "需要本机 ffmpeg"]
fn 导出带空档的两段序列() {
    let dir = scratch("export-e2e");
    let clip = make_clip(&dir);

    let mut project = Project::new();
    project.sequence.width = 320;
    project.sequence.height = 240;
    let asset = project.add_asset(&clip);
    let video = project.sequence.tracks[0].id;
    let audio = project.sequence.tracks[1].id;

    // 视频：0-1 秒、3-4 秒，中间两秒空档应当补黑场。
    for start in [0.0, 3.0] {
        put(&mut project, video, asset, 0.0, 1.0, start);
    }
    put(&mut project, audio, asset, 0.0, 1.0, 0.0);

    let output = run_export(&dir, &project, "out.mp4");
    assert_duration(&output, "00:00:04");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 一次把 xfade / overlay / amix / 黑场 / 静音 / 补齐尾巴都过一遍：这些链在
/// 字符串单测里都「长得对」，但只有 ffmpeg 真连过图才能确认没有类型或标签错误。
#[test]
#[ignore = "需要本机 ffmpeg"]
fn 多轨转场与空档一起导出() {
    let dir = scratch("export-complex");
    let clip = make_clip(&dir);

    let mut project = Project::new();
    project.sequence.width = 320;
    project.sequence.height = 240;
    let asset = project.add_asset(&clip);
    let video0 = project.sequence.tracks[0].id;
    let audio0 = project.sequence.tracks[1].id;

    // 上层视频轨：紧邻的两段做 0.5 秒溶接，再留一段空档，最后又一段画面。
    let first = put(&mut project, video0, asset, 0.0, 1.0, 0.0);
    let second_clip = put(&mut project, video0, asset, 0.6, 1.6, 1.0);
    let transition_id = project.sequence.alloc_id();
    project
        .sequence
        .track_mut(video0)
        .unwrap()
        .transitions
        .push(Transition::new(
            transition_id,
            first,
            second_clip,
            TransitionKind::CrossDissolve,
            Tick::from_millis(500),
        ));
    put(&mut project, video0, asset, 0.0, 1.0, 3.0);

    // 下层视频轨只在 2-3 秒有画面，前后都补黑；两条轨必须等长才能 overlay。
    let video1 = project.sequence.add_track(TrackKind::Video);
    put(&mut project, video1, asset, 0.0, 1.0, 2.0);

    // 两条音频轨各有空档：静音段与片段要能被 concat 接起来，再一起 amix。
    put(&mut project, audio0, asset, 0.0, 1.0, 0.0);
    let audio1 = project.sequence.add_track(TrackKind::Audio);
    put(&mut project, audio1, asset, 0.0, 1.0, 2.0);

    assert_eq!(project.sequence.duration(), second(4.0));
    let output = run_export(&dir, &project, "complex.mp4");
    assert_duration(&output, "00:00:04");
    let _ = std::fs::remove_dir_all(&dir);
}
