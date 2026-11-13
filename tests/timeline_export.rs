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
    AssetId, ClipItem, ExportPlan, ExportSettings, Id, Project, Property, Rational, SequenceSpec,
    SubtitleItem, SubtitleMode, Tick, TrackKind, Transition, TransitionKind, plan_sequence,
    plan_sequence_range, plan_sequence_with,
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
    make_clip_seconds(dir, "clip.mp4", 2.0)
}

/// 指定秒数的素材。变速用例要一段够长的片子，才看得出「时长变一半」。
fn make_clip_seconds(dir: &Path, name: &str, seconds: f64) -> PathBuf {
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
        ])
        .arg(format!("testsrc2=size=320x240:rate=30:duration={seconds}"))
        .args(["-f", "lavfi", "-i"])
        .arg(format!("sine=frequency=1000:duration={seconds}"))
        .args([
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
    let id = project.sequence_mut().alloc_id();
    project
        .sequence_mut()
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
    run_plan(dir, plan, name, project.sequence().duration())
}

/// 跑一条已经算好的计划。区间导出要先自己生成计划（带工作区）。
fn run_plan(dir: &Path, plan: ExportPlan, name: &str, total: Tick) -> PathBuf {
    let output = dir.join(name);

    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let _task = ExportTask::start(plan, output.clone(), total, tx).expect("启动 ffmpeg");

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

// crate root 的 `mod` 默认在 `tests/` 同层找文件，子目录要写全路径。
// 速度曲线那条用例连着它的时长读数工具一起放在 `tests/timeline_export/remap.rs`。
#[path = "timeline_export/remap.rs"]
mod remap;
// 遮罩那条用例（PGM alpha 图 + alphamerge + 黑底）在 `tests/timeline_export/mask.rs`。
#[path = "timeline_export/mask.rs"]
mod mask;
// 混音（推子 / 自动化）的电平核对在 `tests/timeline_export/mixer.rs`。
#[path = "timeline_export/mixer.rs"]
mod mixer;
// 分轨导出（每条音频轨一个 wav）在 `tests/timeline_export/stems.rs`。
#[path = "timeline_export/stems.rs"]
mod stems;

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
    let video = project.sequence_mut().tracks[0].id;
    let audio = project.sequence_mut().tracks[1].id;
    put(&mut project, video, asset, 0.0, 2.0, 0.0);
    put(&mut project, audio, asset, 0.0, 2.0, 0.0);
    assert_eq!(project.sequence_mut().fps, Rational::FPS_25);

    // 只换帧率：片段的时间一个都没动，成片还是两秒。
    project.sequence_mut().fps = Rational::FPS_30;

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
    project.sequence_mut().width = 320;
    project.sequence_mut().height = 240;
    let asset = project.add_asset(&clip);
    let video = project.sequence_mut().tracks[0].id;
    let audio = project.sequence_mut().tracks[1].id;

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
    project.sequence_mut().width = 320;
    project.sequence_mut().height = 240;
    let asset = project.add_asset(&clip);
    let video0 = project.sequence_mut().tracks[0].id;
    let audio0 = project.sequence_mut().tracks[1].id;

    // 上层视频轨：紧邻的两段做 0.5 秒溶接，再留一段空档，最后又一段画面。
    let first = put(&mut project, video0, asset, 0.0, 1.0, 0.0);
    let second_clip = put(&mut project, video0, asset, 0.6, 1.6, 1.0);
    let transition_id = project.sequence_mut().alloc_id();
    project
        .sequence_mut()
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
    let video1 = project.sequence_mut().add_track(TrackKind::Video);
    put(&mut project, video1, asset, 0.0, 1.0, 2.0);

    // 两条音频轨各有空档：静音段与片段要能被 concat 接起来，再一起 amix。
    put(&mut project, audio0, asset, 0.0, 1.0, 0.0);
    let audio1 = project.sequence_mut().add_track(TrackKind::Audio);
    put(&mut project, audio1, asset, 0.0, 1.0, 2.0);

    assert_eq!(project.sequence_mut().duration(), second(4.0));
    let output = run_export(&dir, &project, "complex.mp4");
    assert_duration(&output, "00:00:04");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 两个时刻的画面相似度（PSNR，dB，越大越像）。
///
/// 不用逐字节相同来判定定格：编码器对重复帧会调整 QP / 帧类型，同一个画面解出来
/// 能差几十分之一 dB（实测 60–80 dB），而运动中的画面只有 20 dB 上下。差异足够大，
/// 用 50 dB 当分界既不会误判，也不会漏判。
fn frame_psnr(output: &Path, a: &str, b: &str) -> f64 {
    psnr_between(output, a, output, b)
}

/// 两个文件里各取一帧比对：区间导出要确认「成片的头」对得上「素材的入点」。
fn psnr_between(first: &Path, a: &str, second: &Path, b: &str) -> f64 {
    let filter = format!(
        "[0:v]trim=start={a}:duration=0.04,setpts=PTS-STARTPTS[x];\
         [1:v]trim=start={b}:duration=0.04,setpts=PTS-STARTPTS[y];[x][y]psnr"
    );
    let probe = Command::new("ffmpeg")
        .args(["-hide_banner", "-i"])
        .arg(first)
        .arg("-i")
        .arg(second)
        .arg("-filter_complex")
        .arg(&filter)
        .args(["-f", "null", "-"])
        .output()
        .expect("比对画面");
    let text = String::from_utf8_lossy(&probe.stderr);
    text.split("average:")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("读不到 PSNR：\n{text}"))
}

/// 某个区间的最大音量（dB）。定格段应当接近数字静音。
fn max_volume_db(output: &Path, from: &str, to: &str) -> f64 {
    let filter = format!("atrim=start={from}:end={to},volumedetect");
    let probe = Command::new("ffmpeg")
        .args(["-hide_banner", "-i"])
        .arg(output)
        .args(["-vn", "-af"])
        .arg(&filter)
        .args(["-f", "null", "-"])
        .output()
        .expect("测音量");
    let text = String::from_utf8_lossy(&probe.stderr);
    text.split("max_volume:")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| match value {
            // 纯静音时 volumedetect 给的是 `-inf`，直接 parse 会失败。
            "-inf" => Some(f64::NEG_INFINITY),
            value => value.parse().ok(),
        })
        .unwrap_or_else(|| panic!("读不到音量：\n{text}"))
}

/// M9-T09：10 秒素材 2× 导出应当正好 5 秒，音轨跟着 `atempo` 一起缩短。
#[test]
#[ignore = "需要本机 ffmpeg"]
fn 变速导出时长按倍率缩短() {
    let dir = scratch("export-speed");
    let clip = make_clip_seconds(&dir, "long.mp4", 10.0);

    let mut project = Project::new_with(SequenceSpec::new(320, 240, Rational::FPS_30));
    let asset = project.add_asset(&clip);
    let video = project.sequence_mut().tracks[0].id;
    let audio = project.sequence_mut().tracks[1].id;
    let picture = put(&mut project, video, asset, 0.0, 10.0, 0.0);
    let sound = put(&mut project, audio, asset, 0.0, 10.0, 0.0);

    // 画面与声音都改成 2 倍速：只改一边的话两条轨会不等长，成片被短的那条截断。
    for id in [picture, sound] {
        let ok = ngy_video_player::timeline::apply(
            project.sequence_mut(),
            &ngy_video_player::timeline::EditOp::SetSpeed {
                clip: id,
                speed: 2.0,
            },
        )
        .expect("变速");
        *project.sequence_mut() = ok;
    }
    assert_eq!(
        project.sequence_mut().duration(),
        second(5.0),
        "序列自己先算对"
    );

    let output = run_export(&dir, &project, "speed.mp4");
    assert_duration(&output, "00:00:05");
    assert_frame_rate(&output, "30");
    let _ = std::fs::remove_dir_all(&dir);
}

/// M9-T09：片尾定格把最后一帧拉长，成片变长且定格段是**同一幅**画面。
#[test]
#[ignore = "需要本机 ffmpeg"]
fn 片尾定格延长成片且最后一段静止() {
    let dir = scratch("export-hold");
    let clip = make_clip(&dir);

    let mut project = Project::new();
    project.sequence_mut().width = 320;
    project.sequence_mut().height = 240;
    let asset = project.add_asset(&clip);
    let video = project.sequence_mut().tracks[0].id;
    let audio = project.sequence_mut().tracks[1].id;
    let picture = put(&mut project, video, asset, 0.0, 2.0, 0.0);
    let sound = put(&mut project, audio, asset, 0.0, 2.0, 0.0);

    // 定格 2 秒：画面 `tpad` 克隆最后一帧，声音 `apad` 补等长静音。
    for id in [picture, sound] {
        let ok = ngy_video_player::timeline::apply(
            project.sequence_mut(),
            &ngy_video_player::timeline::EditOp::SetHold {
                clip: id,
                hold: Tick::from_secs(2.0),
            },
        )
        .expect("定格");
        *project.sequence_mut() = ok;
    }
    assert_eq!(project.sequence_mut().duration(), second(4.0));

    let output = run_export(&dir, &project, "hold.mp4");
    assert_duration(&output, "00:00:04");

    // 定格从 2 秒开始：2.5 秒与 3.5 秒必须是同一幅画面。
    let still = frame_psnr(&output, "2.5", "3.5");
    assert!(still > 50.0, "定格段应当视觉静止，实测 {still:.1} dB");
    // 对照：运动段（0.5 ↔ 0.7 秒）必须明显不同，否则上面的阈值毫无意义。
    let moving = frame_psnr(&output, "0.5", "0.7");
    assert!(moving < 30.0, "运动段应当明显不同，实测 {moving:.1} dB");
    // 声音进定格段之后是 `apad` 补的静音，不能还在播原片的声音。
    let quiet = max_volume_db(&output, "2.4", "3.9");
    assert!(quiet < -60.0, "定格段应当静音，实测 {quiet:.1} dB");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 某一时刻画面的平均亮度（`signalstats` 的 `YAVG`，0–255）。
fn average_luma(output: &Path, at: &str) -> f64 {
    let probe = Command::new("ffmpeg")
        .args(["-hide_banner", "-ss", at, "-i"])
        .arg(output)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "signalstats,metadata=print",
            "-f",
            "null",
            "-",
        ])
        .output()
        .expect("测亮度");
    let text = String::from_utf8_lossy(&probe.stderr);
    text.split("lavfi.signalstats.YAVG=")
        .nth(1)
        .and_then(|rest| {
            rest.chars()
                .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
                .collect::<String>()
                .parse()
                .ok()
        })
        .unwrap_or_else(|| panic!("读不到亮度：\n{text}"))
}

/// 生成一个 2 级 identity LUT：套上去画面不该变，但能证明滤镜链真的读到了文件。
fn identity_lut(dir: &Path) -> PathBuf {
    let path = dir.join("identity.cube");
    let mut text = String::from("LUT_3D_SIZE 2\n");
    // .cube 的行顺序是 R 变化最快。
    for b in 0..=1 {
        for g in 0..=1 {
            for r in 0..=1 {
                text.push_str(&format!("{r} {g} {b}\n"));
            }
        }
    }
    std::fs::write(&path, text).expect("写 LUT");
    path
}

/// M9-T11：亮度调高之后成片确实变亮，不是「参数存下来了但没进滤镜链」。
#[test]
#[ignore = "需要本机 ffmpeg"]
fn 调色导出确实改变画面亮度() {
    let dir = scratch("export-color");
    let clip = make_clip(&dir);

    let mut project = Project::new();
    project.sequence_mut().width = 320;
    project.sequence_mut().height = 240;
    let asset = project.add_asset(&clip);
    let video = project.sequence_mut().tracks[0].id;
    let audio = project.sequence_mut().tracks[1].id;
    let picture = put(&mut project, video, asset, 0.0, 2.0, 0.0);
    put(&mut project, audio, asset, 0.0, 2.0, 0.0);

    let plain = run_export(&dir, &project, "plain.mp4");
    project
        .sequence_mut()
        .clip_mut(picture)
        .unwrap()
        .color
        .brightness = 80;
    let bright = run_export(&dir, &project, "bright.mp4");

    let before = average_luma(&plain, "1.0");
    let after = average_luma(&bright, "1.0");
    assert!(
        after > before + 20.0,
        "亮度应当明显上升：{before:.1} → {after:.1}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// M9-T11：LUT 文件真的被 `lut3d` 读进去。identity LUT 不改变画面，
/// 所以只要与不套 LUT 的成片足够像，就说明路径（含 Windows 盘符转义）没写错。
#[test]
#[ignore = "需要本机 ffmpeg"]
fn lut_文件能真正读进滤镜链() {
    let dir = scratch("export-lut");
    let clip = make_clip(&dir);
    let lut = identity_lut(&dir);

    let mut project = Project::new();
    project.sequence_mut().width = 320;
    project.sequence_mut().height = 240;
    let asset = project.add_asset(&clip);
    let video = project.sequence_mut().tracks[0].id;
    let audio = project.sequence_mut().tracks[1].id;
    let picture = put(&mut project, video, asset, 0.0, 2.0, 0.0);
    put(&mut project, audio, asset, 0.0, 2.0, 0.0);

    let plain = run_export(&dir, &project, "plain.mp4");
    project.sequence_mut().clip_mut(picture).unwrap().color.lut = Some(lut);
    let graded = run_export(&dir, &project, "lut.mp4");

    let psnr = psnr_between(&graded, "0.5", &plain, "0.5");
    assert!(psnr > 30.0, "identity LUT 不该改变画面，实测 {psnr:.1} dB");
    let _ = std::fs::remove_dir_all(&dir);
}

/// M9-T13：有入出点时只导那一段，时长与标尺一致，画面从入点接着走。
#[test]
#[ignore = "需要本机 ffmpeg"]
fn 只导出工作区那一段() {
    let dir = scratch("export-range");
    let clip = make_clip_seconds(&dir, "long.mp4", 10.0);

    let mut project = Project::new_with(SequenceSpec::new(320, 240, Rational::FPS_30));
    let asset = project.add_asset(&clip);
    let video = project.sequence_mut().tracks[0].id;
    let audio = project.sequence_mut().tracks[1].id;
    put(&mut project, video, asset, 0.0, 10.0, 0.0);
    put(&mut project, audio, asset, 0.0, 10.0, 0.0);

    let (start, end) = (second(3.0), second(5.0));
    let plan = plan_sequence_range(&project, Some((start, end))).expect("生成计划");
    let output = run_plan(&dir, plan, "range.mp4", end - start);

    assert_duration(&output, "00:00:02");
    // 成片的 0.5 秒应当就是素材的 3.5 秒：入点对齐，而不是从头开始导。
    let aligned = psnr_between(&output, "0.5", &clip, "3.5");
    assert!(aligned > 35.0, "成片与入点对不上，实测 {aligned:.1} dB");
    // 对照：成片 0.5 秒与素材 0.5 秒（另一个位置）必须明显不同，否则上面的相近毫无意义。
    let wrong = psnr_between(&output, "0.5", &clip, "0.5");
    assert!(wrong < 30.0, "对照帧应当不同，实测 {wrong:.1} dB");
    let _ = std::fs::remove_dir_all(&dir);
}

/// M9-T08：推近（`zoompan`）在成片里真的把画面放大了，而且是**逐步**放大。
///
/// 用一块「左三分之一白、其余黑」的静态图案当素材：放大以画面中心为锚点，
/// 白区会逐渐被挤出画面，平均亮度单调下降。只看「滤镜字符串里有 zoompan」不够——
/// 表达式写错（比如少了 `on/fps` 的换算）时字符串依然长得对，画面却不会动。
#[test]
#[ignore = "需要本机 ffmpeg"]
fn 关键帧推近在成片里逐步放大() {
    let dir = scratch("export-zoom");
    let clip = dir.join("steps.mp4");
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i"])
        .arg("color=c=black:size=320x240:rate=30:duration=5,drawbox=x=0:y=0:w=107:h=240:color=white:t=fill")
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-t", "5"])
        .arg(&clip)
        .status()
        .expect("跑 ffmpeg 生成素材");
    assert!(status.success(), "生成测试素材失败");

    let mut project = Project::new_with(SequenceSpec::new(320, 240, Rational::FPS_30));
    let asset = project.add_asset(&clip);
    let video = project.sequence_mut().tracks[0].id;
    let picture = put(&mut project, video, asset, 0.0, 5.0, 0.0);

    // 0 秒 1.0×、5 秒 4.0×：线性插值，2.4 秒时约 2.4×。
    let item = project.sequence_mut().clip_mut(picture).unwrap();
    item.keys.ensure(Property::Scale).insert(Tick::ZERO, 1.0);
    item.keys.ensure(Property::Scale).insert(second(5.0), 4.0);

    let output = run_export(&dir, &project, "zoom.mp4");
    assert_duration(&output, "00:00:05");

    // 默认白色是 235、黑色 16：白区占比从约三分之一逐步掉到 0。
    let first = average_luma(&output, "0.2");
    let middle = average_luma(&output, "2.4");
    let last = average_luma(&output, "4.8");
    assert!(
        first > middle + 20.0,
        "前半段应当还看得见大片白区：{first:.1} → {middle:.1}"
    );
    assert!(
        middle > last + 10.0,
        "后半段白区应当继续缩小：{middle:.1} → {last:.1}"
    );
    assert!(last < 40.0, "4.8 秒时画面应当几乎全黑，实测 {last:.1}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// M9-T10：字幕烧录真的改变画面（只改字幕出现那一段），软字幕真的落成一条
/// 独立的 ASS 轨且不动画面。两者都用同一条序列、同一套编码参数导出，方便逐帧对照。
#[test]
#[ignore = "需要本机 ffmpeg"]
fn 字幕烧录与软字幕导出() {
    let dir = scratch("export-subtitle");
    let clip = make_clip_seconds(&dir, "source.mp4", 4.0);

    let mut project = Project::new_with(SequenceSpec::new(320, 240, Rational::FPS_30));
    let asset = project.add_asset(&clip);
    let video = project.sequence_mut().tracks[0].id;
    let audio = project.sequence_mut().tracks[1].id;
    put(&mut project, video, asset, 0.0, 4.0, 0.0);
    put(&mut project, audio, asset, 0.0, 4.0, 0.0);

    let plain = run_export(&dir, &project, "plain.mp4");

    // 字幕轨：1–3 秒一句，文本与样式都要进 ASS。
    let track = project.sequence_mut().add_track(TrackKind::Subtitle);
    let id = project.sequence_mut().alloc_id();
    project
        .sequence_mut()
        .track_mut(track)
        .unwrap()
        .subtitles
        .push(SubtitleItem::new(id, second(1.0), second(3.0), "字幕测试"));
    let total = project.sequence_mut().duration();

    let burn_plan =
        plan_sequence_with(&project, None, &ExportSettings::default()).expect("烧录计划");
    assert_eq!(
        burn_plan.subtitle.as_ref().unwrap().mode,
        SubtitleMode::Burn
    );
    let burn = run_plan(&dir, burn_plan, "burn.mp4", total);
    assert_duration(&burn, "00:00:04");
    let inside = psnr_between(&plain, "2.0", &burn, "2.0");
    assert!(
        inside < 40.0,
        "字幕出现的 2 秒处画面该明显不同：{inside:.1} dB"
    );
    let outside = psnr_between(&plain, "3.5", &burn, "3.5");
    assert!(outside > 35.0, "字幕之外的 3.5 秒不该变：{outside:.1} dB");

    let soft_settings = ExportSettings {
        subtitle: SubtitleMode::Soft,
        ..ExportSettings::default()
    };
    let soft_plan = plan_sequence_with(&project, None, &soft_settings).expect("软字幕计划");
    let soft = run_plan(&dir, soft_plan, "soft.mkv", total);
    assert_duration(&soft, "00:00:04");
    let probe = Command::new("ffmpeg")
        .args(["-hide_banner", "-i"])
        .arg(&soft)
        .output()
        .expect("读回字幕轨");
    let info = String::from_utf8_lossy(&probe.stderr);
    assert!(
        info.contains("Subtitle: ass"),
        "成片里应当有一条 ASS 软字幕：\n{info}"
    );
    let untouched = psnr_between(&plain, "2.0", &soft, "2.0");
    assert!(untouched > 30.0, "软字幕不该改画面：{untouched:.1} dB");

    let _ = std::fs::remove_dir_all(&dir);
}
