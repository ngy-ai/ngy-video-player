//! 混音（M10-T14）的成片验证：推子与音量自动化在成片里到底降了多少 dB。
//!
//! 模型单测只能证明「公式算得对」，证明不了 `volume` 滤镜真的被接进链里、`eval=frame`
//! 的表达式时间基准也没算错。这里拿一段正弦波真导一遍，量 RMS 比对。

use super::*;
use ngy_video_player::timeline::mixer::GainPoint;

/// 1 kHz 正弦，满幅（-1..1），48kHz 单声道。用 wav 避免编码损失干扰电平。
fn make_sine(dir: &Path, seconds: f64) -> PathBuf {
    let clip = dir.join("sine.wav");
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            &format!("sine=frequency=1000:sample_rate=48000:duration={seconds}"),
            "-c:a",
            "pcm_s16le",
        ])
        .arg(&clip)
        .status()
        .expect("跑 ffmpeg 生成正弦素材");
    assert!(status.success(), "生成正弦素材失败");
    clip
}

/// 2 秒黑场 + 2 秒正弦的工程。
///
/// 光有音频轨导不出来（`ExportError::NoVideoTrack`：空视频轨不算「有画面」），
/// 所以配一段黑场，音频这边才是唯一的变量。
fn project_with_sine(dir: &Path) -> (Project, Id) {
    let black = dir.join("black.mp4");
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:size=320x240:rate=30:duration=2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&black)
        .status()
        .expect("跑 ffmpeg 生成黑场");
    assert!(status.success(), "生成黑场素材失败");

    let mut project = Project::new_with(SequenceSpec::new(320, 240, Rational::FPS_30));
    let video_asset = project.add_asset(&black);
    let audio_asset = project.add_asset(&make_sine(dir, 2.0));
    let video = project.sequence_mut().tracks[0].id;
    let audio = project.sequence_mut().tracks[1].id;
    put(&mut project, video, video_asset, 0.0, 2.0, 0.0);
    put(&mut project, audio, audio_asset, 0.0, 2.0, 0.0);
    (project, audio)
}

#[test]
#[ignore = "需要本机 ffmpeg"]
fn 推子降六_db_成片电平跟着降() {
    let dir = scratch("mixer-fader");
    let (mut project, audio) = project_with_sine(&dir);
    let unity = rms(&run_export(&dir, &project, "mixer-unity.mp4"), 0.5, 1.0);

    // -6 dB（0.5012）推子。
    project.sequence_mut().track_mut(audio).unwrap().gain = 0.5012;
    let lowered = rms(&run_export(&dir, &project, "mixer-minus6.mp4"), 0.5, 1.0);

    let ratio = lowered / unity;
    let db = 20.0 * ratio.log10();
    assert!(
        (db + 6.0).abs() < 1.0,
        "推子 -6 dB 之后电平是 {db:.2} dB（原 {unity:.4} → {lowered:.4}）"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "需要本机 ffmpeg"]
fn 自动化把电平按曲线压下去() {
    let dir = scratch("mixer-auto");
    let (mut project, audio) = project_with_sine(&dir);
    {
        let track = project.sequence_mut().track_mut(audio).unwrap();
        // 0 秒处 1.0，2 秒处 0.25（-12 dB）：线性插值，末尾比开头低 12 dB。
        track.automation = vec![
            GainPoint::new(Tick::from_secs(0.0), 1.0),
            GainPoint::new(Tick::from_secs(2.0), 0.25),
        ];
    }
    let output = run_export(&dir, &project, "mixer-auto.mp4");
    let head = rms(&output, 0.0, 0.5);
    let tail = rms(&output, 1.5, 0.5);
    // 末尾 0.5 秒的平均倍率约 0.44，开头 0.5 秒约 0.94：比值约 2.1（-6.5 dB）。
    let ratio = head / tail;
    let expected = 12.0 * 0.75 / (1.0 - 0.0625); // 见下面的推导
    let db = 20.0 * ratio.log10();
    assert!(
        db > 4.0 && db < 9.0,
        "首尾电平差 {db:.2} dB（开头 {head:.4} → 末尾 {tail:.4}），预期约 {expected:.1} dB"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 从成片里取 `[start, start+duration)` 的 RMS（按 48kHz 单声道解码）。
fn rms(output: &Path, start: f64, duration: f64) -> f32 {
    let result = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error"])
        .arg("-ss")
        .arg(format!("{start:.3}"))
        .arg("-t")
        .arg(format!("{duration:.3}"))
        .arg("-i")
        .arg(output)
        .args([
            "-map", "0:a", "-ac", "1", "-ar", "48000", "-f", "f32le", "-",
        ])
        .output()
        .expect("解码音频");
    let bytes = result.stdout;
    assert!(!bytes.is_empty(), "没有解出音频：{:?}", result.stderr);
    let samples: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    assert!(!samples.is_empty(), "采样点为空");
    let sum: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}
