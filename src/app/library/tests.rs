//! 素材库的纯函数：规格文案、封面尺寸、导入进度、拖到轨道的落点与排布。
//! 面板本身要靠窗口，不在这里测。

use super::covers::{cover_size, fps_label, render_cover_in, spec};
use super::drop::{
    AssetDragItem, all_fit, fits, lane_offset, place_items, reject_reason, snap_start,
    snap_to_clip_edges, total_duration,
};
use super::panel::LibraryImport;
use crate::timeline::{AssetId, MediaInfo, Rational, Sequence, Tick, TrackKind};

#[test]
fn 规格按有无画面分开写() {
    let video = MediaInfo {
        width: 1920,
        height: 1080,
        fps: Some(crate::timeline::Rational::FPS_30),
        ..MediaInfo::default()
    };
    assert_eq!(spec(&video), "1920×1080 · 30fps");
    assert_eq!(
        fps_label(crate::timeline::Rational::FPS_29_97),
        "29.97",
        "23.976 / 29.97 这类不能四舍五入成整数"
    );

    // 手机竖拍：文件里是横的，显示按旋转后的尺寸。
    let portrait = MediaInfo {
        width: 1920,
        height: 1080,
        rotation: 90,
        ..MediaInfo::default()
    };
    assert_eq!(spec(&portrait), "1080×1920");

    let audio = MediaInfo {
        channels: 2,
        sample_rate: 48_000,
        ..MediaInfo::default()
    };
    assert_eq!(spec(&audio), "48.0kHz · 2 声道");
    assert_eq!(
        spec(&MediaInfo {
            channels: 2,
            sample_rate: 44_100,
            ..MediaInfo::default()
        }),
        "44.1kHz · 2 声道",
        "44.1 不能舍成 44"
    );
    assert_eq!(spec(&MediaInfo::default()), "无可用的音视频轨");
}

#[test]
fn 封面只缩小不放大() {
    assert_eq!(cover_size(160, 90), (160, 90));
    assert_eq!(cover_size(80, 45), (80, 45), "小图原样保留");
    // 竖拍：宽度压到 160，高度按比例跟下来。
    assert_eq!(cover_size(320, 480), (160, 240));
    assert_eq!(cover_size(0, 0), (1, 1), "坏尺寸给个能画的方块");
}

#[test]
fn 导入进度按完成数判定结束() {
    let mut import = LibraryImport {
        total: 2,
        done: 0,
        failed: 0,
    };
    assert!(!import.finished());
    import.done = 1;
    import.failed = 1;
    assert!(!import.finished());
    import.done = 2;
    assert!(import.finished(), "失败也算跑完，不能卡在进度条上");
}

// 需要本机 ffmpeg：`cargo test -- --ignored`。
#[test]
#[ignore = "需要本机 ffmpeg：cargo test -- --ignored"]
fn 真实视频能抽出封面() {
    let dir = std::env::temp_dir().join(format!("ngy-lib-cover-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建临时目录");
    let file = dir.join("clip.mp4");

    let executable = crate::platform::ffmpeg::executable().expect("本机没有 ffmpeg");
    let status = std::process::Command::new(&executable)
        .args([
            "-hide_banner",
            "-nostdin",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=2:size=320x240:rate=25",
            "-c:v",
            "mpeg4",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&file)
        .status()
        .expect("生成测试素材");
    assert!(status.success(), "ffmpeg 生成素材失败");

    let image = render_cover_in(&file, &dir.join("cache"))
        .expect("生成封面")
        .expect("两秒的素材应当有第一帧");
    let size = image.size(0);
    // 320×240 缩到 160 宽，高度按比例。
    assert_eq!((i32::from(size.width), i32::from(size.height)), (160, 120));

    let _ = std::fs::remove_dir_all(&dir);
}

fn s(secs: f64) -> Tick {
    Tick::from_secs(secs)
}

fn item(id: u64, secs: f64, video: bool, audio: bool) -> AssetDragItem {
    AssetDragItem {
        id: AssetId::new(id),
        name: format!("素材{id}.mp4"),
        duration: s(secs),
        has_video: video,
        has_audio: audio,
    }
}

fn sequence() -> Sequence {
    Sequence::new("测试", Rational::FPS_25, 1920, 1080)
}

#[test]
fn 素材按有无画面判定能不能放上轨道() {
    let video = item(1, 6.0, true, true);
    let audio = item(2, 4.0, false, true);

    assert!(fits(TrackKind::Video, &video));
    assert!(
        fits(TrackKind::Audio, &video),
        "视频素材带音轨，放音频轨只取声音"
    );
    assert!(!fits(TrackKind::Video, &audio), "纯音频不能进视频轨");
    assert!(fits(TrackKind::Audio, &audio));
    assert!(!fits(TrackKind::Subtitle, &video), "字幕轨还不接拖拽");

    assert!(
        !all_fit(TrackKind::Video, &[video.clone(), audio.clone()]),
        "混拖整组拒绝"
    );
    assert!(all_fit(TrackKind::Video, &[video]));
    assert!(!all_fit(TrackKind::Video, &[]), "空拖拽不算可放");

    let reason = reject_reason(TrackKind::Video, &[audio]);
    assert!(
        reason.contains("只有声音"),
        "拒绝理由要能告诉用户往哪放：{reason}"
    );
}

#[test]
fn 多素材按顺序首尾相接() {
    let base = sequence();
    let track = base.tracks[0].id;
    let items = vec![
        item(1, 2.0, true, false),
        item(2, 3.0, true, false),
        item(3, 1.5, true, false),
    ];

    let next = place_items(&base, track, s(1.0), &items).expect("放得下");
    let clips = &next.tracks[0].clips;
    assert_eq!(clips.len(), 3);
    assert_eq!(clips[0].start, s(1.0));
    assert_eq!(clips[0].end(), s(3.0));
    assert_eq!(clips[1].start, s(3.0), "第二个紧接第一个");
    assert_eq!(clips[2].start, s(6.0));
    assert_eq!(total_duration(&items), s(6.5));
    assert_eq!(next.duration(), s(7.5), "第一个从 1 秒起，整条到 7.5 秒");
}

#[test]
fn 有一段落不下就整组放弃() {
    let base = sequence();
    let track = base.tracks[0].id;
    // 先占住 0..4 秒。
    let occupied =
        place_items(&base, track, Tick::ZERO, &[item(9, 4.0, true, false)]).expect("先放一段");

    // 前一段放得下（3..4 秒压在已有片段上，其实放不下）——整组都要失败。
    let items = vec![item(1, 1.0, true, false), item(2, 1.0, true, false)];
    assert!(
        place_items(&occupied, track, s(3.0), &items).is_err(),
        "落点压在已有片段上就整组不放，不能只进去一段"
    );
    assert_eq!(occupied.tracks[0].clips.len(), 1, "原序列不受影响");

    // 换个空位就放得下。
    let next = place_items(&occupied, track, s(5.0), &items).expect("空白处放得下");
    assert_eq!(next.tracks[0].clips.len(), 3);
}

#[test]
fn 落点离片段边缘近就贴上去() {
    let base = sequence();
    let track_id = base.tracks[0].id;
    let occupied =
        place_items(&base, track_id, s(4.0), &[item(1, 2.0, true, false)]).expect("先放一段");
    let track = &occupied.tracks[0];

    assert_eq!(
        snap_to_clip_edges(s(3.97), track, s(0.1)),
        s(4.0),
        "差 3 帧贴到左边缘，不然会判成重叠"
    );
    assert_eq!(
        snap_to_clip_edges(s(6.05), track, s(0.1)),
        s(6.0),
        "贴右边缘"
    );
    assert_eq!(
        snap_to_clip_edges(s(9.0), track, s(0.1)),
        s(9.0),
        "太远就不动"
    );
    assert_eq!(
        snap_to_clip_edges(s(5.0), track, s(0.1)),
        s(5.0),
        "落在片段中间时两边都够不着，交给重叠检查拒绝"
    );
    assert_eq!(
        snap_to_clip_edges(s(3.97), track, Tick::ZERO),
        s(3.97),
        "容差为 0 时不吸附"
    );
}

#[test]
fn 落点换算与吸附() {
    assert_eq!(lane_offset(100.0, 40.0), 60.0);
    assert_eq!(lane_offset(10.0, 40.0), 0.0, "拖到轨道左边缘外按 0 算");

    let frame = Tick::from_frames(25, Rational::FPS_25);
    assert_eq!(frame, s(1.0), "25fps 下第 25 帧就是 1 秒");
    assert_eq!(snap_start(1.001, Rational::FPS_25), frame);
    assert_eq!(
        snap_start(-0.5, Rational::FPS_25),
        Tick::ZERO,
        "负时间夹到 0"
    );
}
