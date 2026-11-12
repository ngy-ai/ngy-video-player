//! 长片压测（M10-T21）：用代码拼大时间线，压存档、视图计算与导出计划。
//!
//! 两档：
//!
//! * **300 片段 / 2 小时 / 1080p**——日常的「长片」；
//! * **1000 片段 / 3 小时 / 4K**——验收里写的那种工程。
//!
//! 这里不测出图与帧率（那要真素材、真窗口、真解码，属于手工长跑），只压**结构**这几条：
//! 保存与打开是否还在「3 秒以内」的量级、缩放与吸附是否随片段数爆炸、撤销栈是否记得住
//! 上限、自动保存写盘要多久、导出计划生成得出来。素材落成临时小文件（内容无所谓），
//! 所以任何机器上都能跑，也不占空间。

use ngy_video_player::timeline::{
    AssetId, ClipItem, Id, Project, Rational, Sequence, Tick, TrackKind, UndoHistory, ViewState,
    visible_frames, visible_ticks,
};

/// 2 小时 / 4 轨 / 300 片段：4 条轨各 75 段，每段 96 秒。
const CLIPS_PER_TRACK: usize = 75;
const CLIP_SECONDS: f64 = 96.0;

/// 验收里写的那一档：3 小时 / 4K / 1000 片段（4 轨 × 250 段 × 43.2 秒）。
const HEAVY_CLIPS_PER_TRACK: usize = 250;

fn big_project() -> Project {
    heavy_project(CLIPS_PER_TRACK, CLIP_SECONDS, 1080)
}

/// 拼一条 `per_track × 4` 段的时间线。每段等长、首尾相接、素材轮流复用。
fn heavy_project(per_track: usize, clip_seconds: f64, height: u32) -> Project {
    let mut project = Project::new();
    // 素材落成真文件（内容不重要）：导出计划会检查「素材还在不在」，存档只看路径。
    let dir = std::env::temp_dir().join("ngy-stress-media");
    let _ = std::fs::create_dir_all(&dir);
    let assets: Vec<AssetId> = (0..6)
        .map(|index| {
            let path = dir.join(format!("素材 {index}.mp4"));
            if !path.exists() {
                let _ = std::fs::write(&path, b"stress");
            }
            project.add_asset(&path)
        })
        .collect();

    let width = height * 16 / 9;
    let mut sequence = Sequence::new("长片", Rational::FPS_25, width, height);
    sequence.tracks.clear();
    for (track_index, kind) in [
        TrackKind::Video,
        TrackKind::Video,
        TrackKind::Audio,
        TrackKind::Audio,
    ]
    .into_iter()
    .enumerate()
    {
        let track_id = sequence.add_track(kind);
        let ids: Vec<Id> = (0..per_track).map(|_| sequence.alloc_id()).collect();
        let Some(track) = sequence.track_mut(track_id) else {
            continue;
        };
        track.name = format!("{} {}", kind.label(), track_index + 1);
        for (index, id) in ids.into_iter().enumerate() {
            let start = Tick::from_secs(index as f64 * clip_seconds);
            track.clips.push(ClipItem::new(
                id,
                assets[(index + track_index) % assets.len()],
                Tick::ZERO,
                Tick::from_secs(clip_seconds),
                start,
            ));
        }
    }
    let sequence_id = sequence.id;
    project.sequences = vec![sequence];
    project.active_sequence = sequence_id;
    project
}

fn temp_project_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("ngy-stress");
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name)
}

#[test]
fn 两小时四轨三百片段_存档与打开在预算内() {
    let mut project = big_project();
    let sequence = project.sequence();
    assert_eq!(
        sequence
            .tracks
            .iter()
            .map(|track| track.clips.len())
            .sum::<usize>(),
        CLIPS_PER_TRACK * 4
    );
    // 2 小时：75 段 × 96 秒 = 7200 秒。
    let duration = sequence.duration();
    assert!(
        (duration.secs() - 7200.0).abs() < 1.0,
        "压测时间线该是 2 小时，实际 {}",
        duration.secs()
    );

    let path = temp_project_path("stress.ngyproj");
    let started = std::time::Instant::now();
    project.save(&path).expect("保存");
    let save_ms = started.elapsed().as_millis();
    let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);

    let started = std::time::Instant::now();
    let opened = Project::open(&path).expect("打开");
    let open_ms = started.elapsed().as_millis();

    println!(
        "2h/4轨/300 片段：保存 {save_ms}ms（{} KB），打开 {open_ms}ms",
        size / 1024
    );
    // debug 构建（无优化）也留一倍余量：真跑下来远在这条线以下。
    assert!(save_ms < 2000, "保存花了 {save_ms}ms，超过 2 秒");
    assert!(open_ms < 2000, "打开花了 {open_ms}ms，超过 2 秒");
    assert_eq!(opened.sequence().duration(), duration, "打开后时长要一致");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn 三小时四k一千片段的工程能打开() {
    let mut project = heavy_project(HEAVY_CLIPS_PER_TRACK, 43.2, 2160);
    let duration = {
        let sequence = project.sequence();
        assert_eq!(
            sequence
                .tracks
                .iter()
                .map(|track| track.clips.len())
                .sum::<usize>(),
            HEAVY_CLIPS_PER_TRACK * 4
        );
        sequence.duration()
    };
    assert!(
        (duration.secs() - 10_800.0).abs() < 1.0,
        "该是 3 小时，实际 {}",
        duration.secs()
    );

    let path = temp_project_path("stress-4k.ngyproj");
    let started = std::time::Instant::now();
    project.save(&path).expect("保存 4K 长片工程");
    let save_ms = started.elapsed().as_millis();
    let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
    let started = std::time::Instant::now();
    let opened = Project::open(&path).expect("打开 4K 长片工程");
    let open_ms = started.elapsed().as_millis();

    println!(
        "3h/4K/1000 片段：保存 {save_ms}ms（{} KB），打开 {open_ms}ms",
        size / 1024
    );
    assert!(save_ms < 3000, "保存花了 {save_ms}ms，超过 3 秒");
    assert!(open_ms < 3000, "打开花了 {open_ms}ms，超过 3 秒");
    assert_eq!(opened.sequence().duration(), duration);

    // 导出计划也要生成得出来：1000 段拼滤镜链是这一步里最重的一次纯计算。
    let started = std::time::Instant::now();
    let plan = ngy_video_player::timeline::plan_sequence_with(
        &project,
        None,
        &ngy_video_player::timeline::ExportSettings::default(),
    )
    .expect("1000 段也要能生成导出计划");
    let plan_ms = started.elapsed().as_millis();
    println!(
        "3h/4K/1000 片段导出计划：{plan_ms}ms，滤镜链 {} KB",
        plan.filter.len() / 1024
    );
    assert!(plan_ms < 3000, "导出计划花了 {plan_ms}ms");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn 缩放与吸附不随片段数爆炸() {
    let project = big_project();
    let sequence = project.sequence();
    // 全部片段边界当吸附目标：这正是「拖片段时每一帧都要算」的那份数据。
    let mut targets: Vec<Tick> = Vec::with_capacity(CLIPS_PER_TRACK * 8);
    for track in &sequence.tracks {
        for clip in &track.clips {
            targets.push(clip.start);
            targets.push(clip.end());
        }
    }

    let mut view = ViewState::new();
    view.set_width(1600.0, sequence.duration());
    let started = std::time::Instant::now();
    for zoom in [2.0_f32, 20.0, 120.0, 600.0] {
        view.set_zoom(zoom, 4000.0, sequence.duration());
        for _ in 0..50 {
            let ticks = visible_ticks(&view);
            assert!(!ticks.is_empty(), "标尺上总得有刻度");
            let frames = visible_frames(&view, sequence.fps);
            assert!(frames.start() <= frames.end());
            let _ = ngy_video_player::timeline::snap_to_targets(
                &view,
                Tick::from_secs(1234.5),
                &targets,
            );
        }
    }
    let elapsed_ms = started.elapsed().as_millis();
    println!("缩放 / 标尺 / 吸附 200 次：{elapsed_ms}ms");
    assert!(elapsed_ms < 500, "视图计算 200 次花了 {elapsed_ms}ms");
}

#[test]
fn 两小时工程的导出计划与进度分母站得住() {
    let project = big_project();
    let started = std::time::Instant::now();
    let plan = ngy_video_player::timeline::plan_sequence_with(
        &project,
        None,
        &ngy_video_player::timeline::ExportSettings::default(),
    )
    .expect("两小时工程要能生成导出计划");
    let elapsed_ms = started.elapsed().as_millis();
    println!(
        "导出计划：{elapsed_ms}ms，{} 个输入，滤镜链 {} KB",
        plan.inputs.len(),
        plan.filter.len() / 1024
    );
    assert!(elapsed_ms < 2000, "生成导出计划花了 {elapsed_ms}ms");
    // 6 个素材按首次出现顺序去重：不能每个片段都加一条 `-i`。
    assert_eq!(plan.inputs.len(), 6, "同一素材只该有一个输入");
    // 进度分母是序列时长（2 小时）：导出进度条靠它算百分比。
    assert!((project.sequence().duration().secs() - 7200.0).abs() < 1.0);
}

#[test]
fn 撤销栈顶到上限只留最近两百步() {
    let project = big_project();
    let mut history = UndoHistory::new(project.sequence().clone());
    let limit = history.limit();
    assert_eq!(limit, 200, "上限是 200 步（见 timeline::undo）");

    // 连续 250 次「移动播放头」之外的真编辑：每次都把第一段的入点往前挪一帧。
    for step in 0..250 {
        let mut next = history.current().clone();
        if let Some(track) = next.tracks.first_mut()
            && let Some(clip) = track.clips.first_mut()
        {
            clip.source_in = Tick::from_micros(step as i64 * 40_000);
        }
        history.push_labeled(next, format!("第 {step} 步"));
    }

    let depth = history.undo_steps();
    assert!(depth <= limit, "可撤销步数 {depth} 超过上限 {limit}");
    // 一路撤销到底不会崩，也不会退到上限以外。
    let mut undone = 0;
    while history.undo().is_some() {
        undone += 1;
        assert!(undone <= limit, "撤销次数超过上限");
    }
    assert_eq!(undone, depth, "能撤销的次数要等于栈深");
    println!("撤销栈：上限 {limit} 步，实际可撤销 {depth} 步，全部撤销 {undone} 次");
}

#[test]
fn 自动保存写一份两小时工程要多久() {
    let project = big_project();
    let path = temp_project_path("stress-autosave.ngyproj");
    let started = std::time::Instant::now();
    let written = ngy_video_player::timeline::autosave::write(&project, &path).expect("写自动保存");
    let elapsed_ms = started.elapsed().as_millis();
    println!(
        "自动保存两小时工程：{elapsed_ms}ms -> {}",
        written.display()
    );
    assert!(elapsed_ms < 1000, "自动保存花了 {elapsed_ms}ms");
    // 崩溃恢复的依据就是这个文件：能认出来，也能读回来。
    assert_eq!(
        ngy_video_player::timeline::autosave::pending(&path),
        Some(written.clone())
    );
    let opened = Project::open(&written).expect("读回自动保存");
    assert_eq!(opened.sequence().duration(), project.sequence().duration());
    ngy_video_player::timeline::autosave::clear(&path);
    let _ = std::fs::remove_file(&written);
}
