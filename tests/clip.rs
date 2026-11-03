//! 裁切任务的端到端验证。
//!
//! 需要本机有 ffmpeg，默认忽略：CI 里没有它，而这两条测试本身也只是在验证
//! 「参数拼对了、进度回来了」——纯逻辑部分在 `src/clip/export.rs` 的单测里。

use futures::StreamExt as _;
use ngy_video_player::clip::{
    ClipEvent, ClipJob, ClipOutcome, ClipRange, ClipRequest, ConcatEvent, ConcatJob, ConcatOutcome,
    ConcatRequest, CutKind, ExportMode, ExportOptions, OutputFormat,
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

/// 生成一段带一条 srt 字幕轨的视频，用来验证烧录。
fn sample_with_subtitle(ffmpeg: &Path, dir: &Path) -> PathBuf {
    let subtitle = dir.join("sub.srt");
    std::fs::write(
        &subtitle,
        "1\n00:00:00,500 --> 00:00:02,000\n你好\n\n2\n00:00:02,000 --> 00:00:03,500\n世界\n",
    )
    .unwrap();
    let input = dir.join("in-sub.mkv");
    let status = Command::new(ffmpeg)
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
        ])
        .arg("-i")
        .arg(&subtitle)
        .args([
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-map",
            "2:s",
            "-c:v",
            "libx264",
            "-c:a",
            "aac",
            "-c:s",
            "srt",
            "-shortest",
        ])
        .arg(&input)
        .status()
        .expect("生成带字幕的测试视频失败");
    assert!(status.success());
    input
}

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 烧录内置字幕() {
    let ffmpeg = ngy_video_player::platform::ffmpeg::executable().expect("本机没有 ffmpeg");
    let dir = std::env::temp_dir().join(format!("ngy-clip-burn-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = sample_with_subtitle(&ffmpeg, &dir);

    let output = dir.join("burned.mkv");
    let request = ClipRequest {
        input,
        output: output.clone(),
        range: ClipRange {
            start: 0.5,
            end: 3.0,
        },
        options: ExportOptions::default(),
        burn: Some(ngy_video_player::clip::BurnSubtitle {
            stream: 0,
            name: "SRT".to_string(),
        }),
        loudness: None,
        kind: CutKind::Keep,
        duration: 4.0,
    };
    let (outcome, _progress) = run(request);

    match outcome {
        ClipOutcome::Finished(paths) => assert_eq!(paths, vec![output.clone()]),
        other => panic!("烧录字幕失败: {other:?}"),
    }
    assert_output(&output);
    // 烧录之后容器里不该再有字幕流。
    let streams = Command::new(&ffmpeg)
        .args(["-hide_banner", "-i"])
        .arg(&output)
        .output()
        .expect("探测输出文件失败");
    let probe = String::from_utf8_lossy(&streams.stderr).into_owned();
    assert!(
        !probe.contains("Subtitle:"),
        "字幕应当已经烧进画面：{probe}"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

/// 用 ffmpeg 探测文件时长（秒）。
fn probe_seconds(ffmpeg: &Path, path: &Path) -> f64 {
    let output = Command::new(ffmpeg)
        .args(["-hide_banner", "-i"])
        .arg(path)
        .output()
        .expect("探测输出文件失败");
    let text = String::from_utf8_lossy(&output.stderr).into_owned();
    let start = text.find("Duration: ").expect("没有时长信息") + "Duration: ".len();
    // 形如 `00:00:03.05`，固定 11 个字符。
    let stamp = &text[start..start + 11];
    let parts: Vec<f64> = stamp
        .split(':')
        .filter_map(|part| part.parse().ok())
        .collect();
    assert_eq!(parts.len(), 3, "解析时长失败：{stamp}");
    parts[0] * 3600.0 + parts[1] * 60.0 + parts[2]
}

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 删除中间段之后首尾接上() {
    let ffmpeg = ngy_video_player::platform::ffmpeg::executable().expect("本机没有 ffmpeg");
    let dir = std::env::temp_dir().join(format!("ngy-clip-cutout-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = sample(&ffmpeg, &dir);

    let output = dir.join("cut.mkv");
    let request = ClipRequest {
        input,
        output: output.clone(),
        range: ClipRange {
            start: 1.0,
            end: 2.0,
        },
        options: ExportOptions::default(),
        burn: None,
        loudness: None,
        kind: CutKind::RemoveMiddle,
        duration: 4.0,
    };
    let (outcome, _progress) = run(request);

    match outcome {
        ClipOutcome::Finished(paths) => assert_eq!(paths, vec![output.clone()]),
        other => panic!("删除中间段失败: {other:?}"),
    }
    assert_output(&output);
    // 4 秒去掉 1 秒，剩下约 3 秒。
    let seconds = probe_seconds(&ffmpeg, &output);
    assert!((2.5..3.5).contains(&seconds), "剩余时长不对：{seconds}");

    std::fs::remove_dir_all(&dir).unwrap();
}

fn run_concat(request: ConcatRequest) -> ConcatOutcome {
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let _job = ConcatJob::start(request, tx).expect("启动拼接失败");
    futures::executor::block_on(async {
        while let Some(event) = rx.next().await {
            if let ConcatEvent::Done(outcome) = event {
                return outcome;
            }
        }
        panic!("任务没有回报结果");
    })
}

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 两个文件按顺序拼成一个() {
    let ffmpeg = ngy_video_player::platform::ffmpeg::executable().expect("本机没有 ffmpeg");
    let dir = std::env::temp_dir().join(format!("ngy-concat-e2e-{}", std::process::id()));
    let first = dir.join("a");
    let second = dir.join("b");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    let a = sample(&ffmpeg, &first);
    let b = sample(&ffmpeg, &second);

    let output = dir.join("joined.mkv");
    let outcome = run_concat(ConcatRequest {
        inputs: vec![a, b],
        output: output.clone(),
    });
    match outcome {
        ConcatOutcome::Finished(path) => assert_eq!(path, output),
        other => panic!("拼接失败: {other:?}"),
    }
    assert_output(&output);
    // 两段各 4 秒。
    let seconds = probe_seconds(&ffmpeg, &output);
    assert!((7.0..9.0).contains(&seconds), "拼接后时长不对：{seconds}");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 在切点处分成两个文件() {
    let ffmpeg = ngy_video_player::platform::ffmpeg::executable().expect("本机没有 ffmpeg");
    let dir = std::env::temp_dir().join(format!("ngy-clip-split-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = sample(&ffmpeg, &dir);

    let request = ClipRequest {
        output: ngy_video_player::clip::split_output_paths(&input).0,
        input: input.clone(),
        range: ClipRange {
            start: 1.5,
            end: 3.0,
        },
        options: ExportOptions::default(),
        burn: None,
        loudness: None,
        kind: CutKind::Split,
        duration: 4.0,
    };
    let (outcome, _progress) = run(request);

    let paths = match outcome {
        ClipOutcome::Finished(paths) => paths,
        other => panic!("分割失败: {other:?}"),
    };
    assert_eq!(paths.len(), 2, "分割应当写出两个文件");
    for path in &paths {
        assert_output(path);
    }
    // 4 秒在 1.5 秒处切开：前半 1.5 秒、后半 2.5 秒。
    let head = probe_seconds(&ffmpeg, &paths[0]);
    let tail = probe_seconds(&ffmpeg, &paths[1]);
    assert!((1.2..1.8).contains(&head), "前半时长不对：{head}");
    assert!((2.2..2.8).contains(&tail), "后半时长不对：{tail}");

    std::fs::remove_dir_all(&dir).unwrap();
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
        burn: None,
        loudness: None,
        kind: CutKind::Keep,
        duration: 4.0,
    };
    let (outcome, progress) = run(request);

    match outcome {
        ClipOutcome::Finished(paths) => assert_eq!(paths, vec![output.clone()]),
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
        burn: None,
        loudness: None,
        kind: CutKind::Keep,
        duration: 4.0,
    };
    let (outcome, _progress) = run(request);

    match outcome {
        ClipOutcome::Finished(paths) => assert_eq!(paths, vec![output.clone()]),
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
        burn: None,
        loudness: None,
        kind: CutKind::Keep,
        duration: 4.0,
    };
    let (outcome, _progress) = run(request);

    match outcome {
        ClipOutcome::Finished(paths) => assert_eq!(paths, vec![output.clone()]),
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
        burn: None,
        loudness: None,
        kind: CutKind::Keep,
        duration: 4.0,
    };
    let (outcome, _progress) = run(request);

    match outcome {
        ClipOutcome::Finished(paths) => assert_eq!(paths, vec![output.clone()]),
        other => panic!("无损剪切失败: {other:?}"),
    }
    assert_output(&output);

    std::fs::remove_dir_all(&dir).unwrap();
}
