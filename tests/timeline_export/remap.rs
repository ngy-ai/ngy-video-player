//! 速度曲线（时间重映射）的成片验证：总时长等于曲线积分、音轨不跟着漂。
//!
//! 曲线是「素材位置 → 时间线位置」的积分，模型侧用 64 段中点法、成片侧用 setpts
//! 表达式；两边必须落在同一帧里，否则预览与导出会各说各话。

use super::*;
/// M10-T11：速度曲线的成片时长要等于曲线积分（误差 < 1 帧），音轨跟着分段变速。
///
/// 曲线：0→2 秒从 2× 线性降到 0.5×，2→6 秒从 0.5× 升回 1×。总时长是这两段
/// `∫ds/rate`，模型（64 段中点法）与成片（分段 `atempo` + `setpts` 表达式）必须对上。
#[test]
#[ignore = "需要本机 ffmpeg"]
fn 速度曲线的成片时长等于曲线积分() {
    let dir = scratch("export-remap");
    let clip = make_clip_seconds(&dir, "curve.mp4", 6.0);

    let mut project = Project::new_with(SequenceSpec::new(320, 240, Rational::FPS_30));
    let asset = project.add_asset(&clip);
    let video = project.sequence_mut().tracks[0].id;
    let audio = project.sequence_mut().tracks[1].id;
    let picture = put(&mut project, video, asset, 0.0, 6.0, 0.0);
    let sound = put(&mut project, audio, asset, 0.0, 6.0, 0.0);

    let mut curve = ngy_video_player::timeline::SpeedCurve::default();
    curve.insert(0.0, 2.0);
    curve.insert(2.0, 0.5);
    curve.insert(6.0, 1.0);
    // 画面与声音挂同一条曲线：只挂一边的话两条轨不等长，成片被短的那条截断。
    for id in [picture, sound] {
        let next = ngy_video_player::timeline::apply(
            project.sequence_mut(),
            &ngy_video_player::timeline::EditOp::SetRemap {
                clip: id,
                curve: curve.clone(),
            },
        )
        .expect("挂曲线");
        *project.sequence_mut() = next;
    }

    let expected = project.sequence().duration().secs();
    // 闭式解：2·ln(4)/1.5 + 4·ln(2)/0.5 ≈ 1.848 + 5.545 = 7.394 秒。
    // 它既不是 6 秒（原速）也不是 3 秒（全程 2×），后面两段各有一半时间在慢放。
    let closed = 2.0 * 4.0f64.ln() / 1.5 + 4.0 * 2.0f64.ln() / 0.5;
    assert!(
        (expected - closed).abs() < 0.01,
        "曲线积分算出来的时长是 {expected} 秒，闭式解 {closed} 秒"
    );

    let output = run_export(&dir, &project, "curve-out.mp4");
    let actual = read_seconds(&output);
    // 一帧（30fps）是 0.033 秒；成片时长按帧落点，容差给一帧。
    assert!(
        (actual - expected).abs() < 1.0 / 30.0,
        "成片 {actual} 秒，曲线积分 {expected} 秒"
    );
    assert_frame_rate(&output, "30");
    // 音轨按折点分段 atempo，长度必须和画面一样：差一帧以上就算漂了。
    let audio = read_audio_seconds(&output);
    assert!(
        (audio - expected).abs() < 1.0 / 30.0,
        "音轨 {audio} 秒，画面 {expected} 秒"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 读回成片时长（秒）。`ffmpeg -i` 的 Duration 行精度是 0.01 秒，够卡一帧的判据。
fn read_seconds(output: &Path) -> f64 {
    let probe = Command::new("ffmpeg")
        .args(["-hide_banner", "-i"])
        .arg(output)
        .output()
        .expect("读回输出");
    let text = String::from_utf8_lossy(&probe.stderr);
    let stamp = text
        .lines()
        .find_map(|line| line.split("Duration:").nth(1))
        .unwrap_or_else(|| panic!("没有 Duration 行：\n{text}"));
    let stamp = stamp.split(',').next().unwrap_or("").trim();
    parse_stamp(stamp).expect("能解析时长")
}

/// `HH:MM:SS.ss` → 秒。
fn parse_stamp(stamp: &str) -> Option<f64> {
    let stamp = stamp.trim();
    let mut parts = stamp.split(':');
    let hours: f64 = parts.next()?.parse().ok()?;
    let minutes: f64 = parts.next()?.parse().ok()?;
    let seconds: f64 = parts.next()?.parse().ok()?;
    (hours.is_finite() && minutes.is_finite() && seconds.is_finite())
        .then_some(hours * 3600.0 + minutes * 60.0 + seconds)
}

/// 单独把音轨解码成空输出，读它自己的时长。
///
/// 容器时长取的是最长的那条流：音频短了在 `Duration` 里看不出来，所以音轨要单独量
/// ——「音画不漂移」这条判据只能这么验。
fn read_audio_seconds(output: &Path) -> f64 {
    let probe = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostats", "-i"])
        .arg(output)
        .args(["-map", "0:a", "-f", "null", "-"])
        .output()
        .expect("解码音轨");
    let text = String::from_utf8_lossy(&probe.stderr);
    // 进度行形如 `frame= ... time=00:00:07.39 bitrate=...`，取最后一条。
    text.lines()
        .rev()
        .filter_map(|line| line.rsplit("time=").next())
        .find_map(|tail| parse_stamp(tail.split_whitespace().next().unwrap_or("")))
        .unwrap_or_else(|| panic!("没有读到音轨时长：\n{text}"))
}
