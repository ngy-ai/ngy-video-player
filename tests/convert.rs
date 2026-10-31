//! 格式转换任务的端到端验证。
//!
//! 需要本机有 ffmpeg，默认忽略：CI 里没有它，而这两条测试本身也只是在验证
//! 「参数拼对了、进度回来了、批量跑完了」——纯逻辑部分在 `src/convert/` 的单测里。

use futures::StreamExt as _;
use ngy_video_player::convert::{ConvertEvent, ConvertJob, ConvertOutcome, TargetFormat, plan};
use std::path::PathBuf;
use std::process::Command;

fn generate(ffmpeg: &std::path::Path, output: &std::path::Path) {
    let status = Command::new(ffmpeg)
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x120:rate=25:duration=2",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
        ])
        .arg(output)
        .status()
        .expect("生成测试视频失败");
    assert!(status.success());
}

#[test]
#[ignore = "需要本机安装 ffmpeg"]
fn 批量转换成_mkv() {
    let ffmpeg = ngy_video_player::platform::ffmpeg::executable().expect("本机没有 ffmpeg");
    let dir = std::env::temp_dir().join(format!("ngy-convert-e2e-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("子目录")).unwrap();

    let first = dir.join("第1集.mp4");
    let second = dir.join("子目录/第2集.mp4");
    generate(&ffmpeg, &first);
    generate(&ffmpeg, &second);

    let mkv = TargetFormat::by_extension("mkv").expect("格式表里应该有 mkv");
    let requests = plan(&[first.clone(), second.clone()], mkv);
    let outputs: Vec<std::path::PathBuf> = requests
        .iter()
        .map(|request| request.output.clone())
        .collect();
    assert_eq!(
        outputs,
        vec![dir.join("第1集.mkv"), dir.join("子目录/第2集.mkv")]
    );

    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let _job = ConvertJob::start(requests, tx).expect("启动转换失败");

    let (outcome, progress, started) = futures::executor::block_on(async {
        let mut progress = Vec::new();
        let mut started = 0;
        while let Some(event) = rx.next().await {
            match event {
                ConvertEvent::ItemStarted { .. } => started += 1,
                ConvertEvent::Progress { ratio, .. } => progress.push(ratio),
                ConvertEvent::Finished(outcome) => return (outcome, progress, started),
                _ => {}
            }
        }
        panic!("任务没有回报结果");
    });

    assert_eq!(started, 2, "两项都应当开始处理");
    match outcome {
        ConvertOutcome::Done(summary) => {
            assert!(summary.failures.is_empty(), "{:?}", summary.failures);
            assert_eq!(summary.outputs, outputs);
            assert_eq!(summary.skipped, 0);
        }
        ConvertOutcome::Cancelled(summary) => panic!("不该被取消: {summary:?}"),
    }
    // 进度有可能一次都没回来（两秒的片子转得很快），回来过一次就必须在合法区间内。
    assert!(
        progress
            .iter()
            .flatten()
            .all(|ratio| (0.0..=1.0).contains(ratio)),
        "{progress:?}"
    );
    for output in &outputs {
        assert!(output.is_file(), "{} 不存在", output.display());
        assert!(std::fs::metadata(output).unwrap().len() > 0);
    }

    std::fs::remove_dir_all(&dir).unwrap();
}

/// 跑完一个批次，返回每一项的失败原因（成功就是空）。
fn run_batch(requests: Vec<ngy_video_player::convert::ConvertRequest>) -> Vec<(PathBuf, String)> {
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let job = ConvertJob::start(requests.clone(), tx).expect("启动转换失败");
    let failures = futures::executor::block_on(async {
        while let Some(event) = rx.next().await {
            if let ConvertEvent::Finished(outcome) = event {
                return match outcome {
                    ConvertOutcome::Done(summary) => summary
                        .failures
                        .into_iter()
                        .map(|failure| (failure.input, failure.message))
                        .collect(),
                    ConvertOutcome::Cancelled(summary) => panic!("不该被取消: {summary:?}"),
                };
            }
        }
        panic!("任务没有回报结果");
    });
    drop(job);
    failures
}

#[test]
#[ignore = "需要本机安装 ffmpeg；要把几十种容器都编码一遍，耗时以分钟计"]
fn 每种目标格式都能写出文件() {
    let ffmpeg = ngy_video_player::platform::ffmpeg::executable().expect("本机没有 ffmpeg");
    let dir = std::env::temp_dir().join(format!("ngy-convert-all-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("源.mp4");
    generate(&ffmpeg, &input);

    // 表格是手写的（容器 → 封装器 → 编码参数），改一行就可能拼错一个参数。
    // 这条测试把整张表跑一遍，失败的容器连同 ffmpeg 的错误一起列出来。
    let mut failed = Vec::new();
    for target in TargetFormat::ALL.iter().copied() {
        let requests = plan(std::slice::from_ref(&input), target);
        let output = requests[0].output.clone();
        let failures = run_batch(requests);
        if let Some((_, message)) = failures.into_iter().next() {
            let reason = message
                .lines()
                .last()
                .unwrap_or(&message)
                .trim()
                .to_string();
            failed.push(format!("{:<5} {reason}", target.extension()));
        } else {
            let size = std::fs::metadata(&output)
                .map(|meta| meta.len())
                .unwrap_or(0);
            if size == 0 {
                failed.push(format!("{:<5} 输出是空文件", target.extension()));
            }
        }
        let _ = std::fs::remove_file(&output);
    }

    std::fs::remove_dir_all(&dir).unwrap();
    assert!(
        failed.is_empty(),
        "这些格式没转出来：\n{}",
        failed.join("\n")
    );
}
