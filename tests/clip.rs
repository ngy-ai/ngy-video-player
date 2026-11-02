//! 裁切任务的端到端验证。
//!
//! 需要本机有 ffmpeg，默认忽略：CI 里没有它，而这两条测试本身也只是在验证
//! 「参数拼对了、进度回来了」——纯逻辑部分在 `src/clip/export.rs` 的单测里。

use futures::StreamExt as _;
use ngy_video_player::clip::{
    ClipEvent, ClipJob, ClipOutcome, ClipRange, ClipRequest, ExportMode, ExportOptions,
    OutputFormat,
};
use std::path::{Path, PathBuf};
use std::process::Command;

/// 生成一段 4 秒、每秒一个关键帧的测试视频。关键帧间隔固定下来，
/// 无损流复制的起点才有确定的落点。
fn sample(ffmpeg: &Path, dir: &Path) -> PathBuf {
    let input = dir.join("in.mp4");
    let generated = Command::new(ffmpeg)
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x120:rate=25:duration=4",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=4",
            "-c:v",
            "libx264",
            "-g",
            "25",
            "-pix_fmt",
            "yuv420p",
            // 带上色彩标签：zscale 靠它判断输入色彩空间，无标签的源上滤镜链
            // 会直接报「no path between colorspaces」。真实片源都有这些标签。
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
            "-c:a",
            "aac",
        ])
        .arg(&input)
        .status()
        .expect("生成测试视频失败");
    assert!(generated.success());
    input
}

fn run(request: ClipRequest) -> (ClipOutcome, Vec<f32>) {
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let _job = ClipJob::start(request, tx).expect("启动裁切失败");
    futures::executor::block_on(async {
        let mut progress = Vec::new();
        while let Some(event) = rx.next().await {
            match event {
                ClipEvent::Progress(value) => progress.push(value),
                ClipEvent::Done(outcome) => return (outcome, progress),
            }
        }
        panic!("任务没有回报结果");
    })
}

fn assert_output(output: &Path) {
    assert!(output.is_file(), "输出文件不存在");
    assert!(std::fs::metadata(output).unwrap().len() > 0);
}

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 裁切一段真实视频() {
    let ffmpeg = ngy_video_player::platform::ffmpeg::executable().expect("本机没有 ffmpeg");
    let dir = std::env::temp_dir().join(format!("ngy-clip-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = sample(&ffmpeg, &dir);

    let output = dir.join("out.mkv");
    let request = ClipRequest {
        input,
        output: output.clone(),
        range: ClipRange {
            start: 1.0,
            end: 3.0,
        },
        options: ExportOptions::default(),
    };
    let (outcome, progress) = run(request);

    match outcome {
        ClipOutcome::Finished(path) => assert_eq!(path, output),
        other => panic!("裁切失败: {other:?}"),
    }
    assert!(!progress.is_empty(), "应当收到进度事件");
    assert!(
        progress.last().is_some_and(|value| *value > 0.0),
        "进度应当往前走"
    );
    assert_output(&output);

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 滤镜与响度处理能被_ffmpeg_接受() {
    let ffmpeg = ngy_video_player::platform::ffmpeg::executable().expect("本机没有 ffmpeg");
    let dir = std::env::temp_dir().join(format!("ngy-clip-filter-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = sample(&ffmpeg, &dir);

    let output = dir.join("filtered.mkv");
    let request = ClipRequest {
        input,
        output: output.clone(),
        range: ClipRange {
            start: 0.5,
            end: 2.5,
        },
        options: ExportOptions {
            tonemap_hdr: true,
            deinterlace: true,
            denoise: true,
            sharpen: true,
            normalize_audio: true,
            // 一并验证恒定帧率、时间戳修复、漂移校正与音频延迟的真实参数。
            force_cfr: true,
            fix_timestamps: true,
            sync_audio: true,
            audio_offset: 0.2,
            ..ExportOptions::default()
        },
    };
    let (outcome, _progress) = run(request);

    match outcome {
        ClipOutcome::Finished(path) => assert_eq!(path, output),
        other => panic!("滤镜链被 ffmpeg 拒绝: {other:?}"),
    }
    assert_output(&output);

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 导出_hls_切片() {
    let ffmpeg = ngy_video_player::platform::ffmpeg::executable().expect("本机没有 ffmpeg");
    let dir = std::env::temp_dir().join(format!("ngy-clip-hls-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = sample(&ffmpeg, &dir);

    let output = dir.join("out.m3u8");
    let request = ClipRequest {
        input,
        output: output.clone(),
        range: ClipRange {
            start: 0.0,
            end: 4.0,
        },
        options: ExportOptions {
            format: OutputFormat::Hls,
            ..ExportOptions::default()
        },
    };
    let (outcome, _progress) = run(request);

    match outcome {
        ClipOutcome::Finished(path) => assert_eq!(path, output),
        other => panic!("HLS 导出失败: {other:?}"),
    }
    assert!(output.is_file(), "播放列表不存在");
    let segments = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".ts"))
        .count();
    assert!(segments > 0, "没有生成任何 .ts 分片");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 无损流复制一段真实视频() {
    let ffmpeg = ngy_video_player::platform::ffmpeg::executable().expect("本机没有 ffmpeg");
    let dir = std::env::temp_dir().join(format!("ngy-clip-copy-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = sample(&ffmpeg, &dir);

    let output = dir.join("copy.mkv");
    let request = ClipRequest {
        input,
        output: output.clone(),
        range: ClipRange {
            start: 1.0,
            end: 3.0,
        },
        options: ExportOptions {
            mode: ExportMode::Copy,
            ..ExportOptions::default()
        },
    };
    let (outcome, _progress) = run(request);

    match outcome {
        ClipOutcome::Finished(path) => assert_eq!(path, output),
        other => panic!("无损剪切失败: {other:?}"),
    }
    assert_output(&output);

    std::fs::remove_dir_all(&dir).unwrap();
}
