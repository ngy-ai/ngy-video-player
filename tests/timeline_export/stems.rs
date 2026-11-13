//! 分轨导出的成片验证：真写出 wav、长度等于序列、推子生效。
//!
//! 计划层只能证明「字符串拼对了」；这里跑真 ffmpeg，确认 `concat` 拼出来的音频
//! 是一条完整的 wav（长度对、电平对），拿去 DAW 能直接用。

use super::*;
use ngy_video_player::timeline::{Tick, plan_stems};

/// 1 kHz 正弦，满幅，48kHz 单声道。
fn make_sine(dir: &Path, name: &str, seconds: f64) -> PathBuf {
    let clip = dir.join(name);
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

#[test]
#[ignore = "需要本机 ffmpeg"]
fn 分轨导出成_wav_且长度等于序列() {
    let dir = scratch("export-stems");
    let sine = make_sine(&dir, "sine.wav", 2.0);

    let mut project = Project::new_with(SequenceSpec::new(320, 240, Rational::FPS_30));
    let asset = project.add_asset(&sine);
    let music = project.sequence().tracks[1].id;
    let voice = project.sequence_mut().add_track(TrackKind::Audio);
    put(&mut project, music, asset, 0.0, 2.0, 0.0);
    put(&mut project, voice, asset, 0.0, 2.0, 0.0);
    // 音乐那条推子拉到 -6 dB，人声保持 0 dB：两条分轨的电平应当差一倍左右。
    project.sequence_mut().track_mut(music).unwrap().gain = 0.5012;

    let total = project.sequence().duration();
    assert_eq!(total, Tick::from_secs(2.0));

    let stems = plan_stems(&project, &dir).expect("能导出分轨");
    assert_eq!(stems.len(), 2);
    let mut outputs = Vec::new();
    for stem in &stems {
        let name = stem
            .path
            .file_name()
            .expect("有文件名")
            .to_string_lossy()
            .into_owned();
        let output = run_plan(&dir, stem.plan.clone(), &name, total);
        assert!(output.exists(), "没写出 {name}");
        let seconds = wav_seconds(&output);
        assert!(
            (seconds - 2.0).abs() < 0.05,
            "{name} 长度 {seconds} 秒，序列是 2 秒"
        );
        outputs.push(output);
    }

    // 推子 0.5 → 峰值减半：音乐轨的 RMS 应当明显低于人声轨。
    let music_rms = rms(&outputs[0]);
    let voice_rms = rms(&outputs[1]);
    let ratio = music_rms / voice_rms;
    assert!(
        (ratio - 0.5012).abs() < 0.05,
        "两条分轨的电平比是 {ratio:.3}（音乐 {music_rms:.4} / 人声 {voice_rms:.4}）"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// wav 的时长：按 PCM 字节数反算。
///
/// 链尾统一 `aformat=channel_layouts=stereo`，所以是 24bit × 2 声道 = 每帧 6 字节。
fn wav_seconds(path: &Path) -> f64 {
    let bytes = std::fs::metadata(path).expect("读文件大小").len();
    // 去掉 44 字节的 wav 头。
    (bytes.saturating_sub(44)) as f64 / 6.0 / 48_000.0
}

/// 解出前 0.5 秒的 RMS（跳过开头的编码器预热）。
fn rms(path: &Path) -> f32 {
    let result = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-ss",
            "0.2",
            "-t",
            "1.0",
            "-i",
        ])
        .arg(path)
        .args(["-ac", "1", "-ar", "48000", "-f", "f32le", "-"])
        .output()
        .expect("解码分轨");
    let bytes = result.stdout;
    assert!(!bytes.is_empty(), "没有解出音频：{:?}", result.stderr);
    let samples: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect();
    let sum: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}
