use super::*;
use crate::timeline::{
    Animation, ClipItem, ColorGrade, Effect, EffectKind, Id, Keyframe, Property, Rational, Tick,
};

/// 一份带素材的工程。渲染计划会检查素材在不在盘上，所以这里得落一个真文件。
fn project() -> (Project, std::path::PathBuf) {
    let dir = std::env::temp_dir().join("ngy-render-cache-tests");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("采访 01.mp4");
    if !path.exists() {
        let _ = std::fs::write(&path, b"cache-test");
    }
    let mut project = Project::new();
    project.add_asset(&path);
    (project, path)
}

fn sequence() -> Sequence {
    let mut sequence = Sequence::new("序列 1", Rational::FPS_30, 1920, 1080);
    sequence.id = Id::new(1);
    sequence
}

fn clip() -> ClipItem {
    ClipItem::new(
        Id::new(10),
        crate::timeline::AssetId::new(1),
        Tick::from_secs(1.0),
        Tick::from_secs(5.0),
        Tick::ZERO,
    )
}

#[test]
fn 只有加了效果调色或关键帧才值得缓存() {
    let mut plain = clip();
    assert!(!worth_caching(&plain), "干净片段不需要缓存");

    plain
        .effects
        .push(Effect::new(Id::new(1), EffectKind::Denoise));
    assert!(worth_caching(&plain), "有降噪就该缓存");

    // 关掉的效果不算，否则用户关一下也得重渲一遍。
    plain.effects[0].enabled = false;
    assert!(!worth_caching(&plain));

    let mut graded = clip();
    graded.color = ColorGrade {
        brightness: 20,
        ..ColorGrade::default()
    };
    assert!(worth_caching(&graded));

    let mut keyed = clip();
    let mut curve = Animation::new(Property::Scale);
    curve.keys.push(Keyframe::new(Tick::ZERO, 1.0));
    curve.keys.push(Keyframe::new(Tick::from_secs(2.0), 2.0));
    keyed.keys.curves.push(curve);
    assert!(worth_caching(&keyed));
}

#[test]
fn 变速与定格不缓存() {
    // 缓存文件的时间轴是「片段自己的 0 秒起」，倍率不是 1 就对不上。
    let mut fast = clip();
    fast.speed = 2.0;
    fast.effects
        .push(Effect::new(Id::new(1), EffectKind::Sharpen));
    assert!(!worth_caching(&fast));

    let mut frozen = clip();
    frozen.hold = Tick::from_secs(1.0);
    frozen
        .effects
        .push(Effect::new(Id::new(1), EffectKind::Sharpen));
    assert!(!worth_caching(&frozen));
}

#[test]
fn 速度曲线可以缓存且进指纹() {
    let (project, _asset_path) = project();
    let sequence = sequence();
    let mut curved = clip();
    curved
        .effects
        .push(Effect::new(Id::new(1), EffectKind::Denoise));
    curved.remap.insert(0.0, 2.0);
    curved.remap.insert(4.0, 0.5);
    // 变速烘焙进缓存文件，片段的第 N 秒还是文件的第 N 秒。
    assert!(worth_caching(&curved), "曲线片段也值得缓存");

    // 曲线变了必须换指纹，否则改完曲线还在放旧的变速结果。
    let base = fingerprint(&project, &sequence, &curved);
    let mut slower = curved.clone();
    slower.remap.insert(4.0, 0.25);
    assert_ne!(base, fingerprint(&project, &sequence, &slower));
}

#[test]
fn 指纹随画面参数变化() {
    let (project, _asset_path) = project();
    let sequence = sequence();
    let mut clip = clip();
    clip.effects
        .push(Effect::new(Id::new(1), EffectKind::Denoise));

    let base = fingerprint(&project, &sequence, &clip);
    assert_eq!(
        base,
        fingerprint(&project, &sequence, &clip),
        "同样的参数必须同指纹"
    );

    // 改强度：同一效果不同参数也得重渲。
    let mut louder = clip.clone();
    louder.effects[0].params.strength = 3;
    assert_ne!(base, fingerprint(&project, &sequence, &louder));

    // 改素材出点。
    let mut longer = clip.clone();
    longer.source_out = Tick::from_secs(6.0);
    assert_ne!(base, fingerprint(&project, &sequence, &longer));

    // 改序列规格：同一个片段在 4K 序列里渲出来的像素不一样。
    let mut big = sequence.clone();
    big.width = 3840;
    big.height = 2160;
    assert_ne!(base, fingerprint(&project, &big, &clip));
}

#[test]
fn 落点带素材名与指纹() {
    let (project, _asset_path) = project();
    let sequence = sequence();
    let mut clip = clip();
    clip.effects
        .push(Effect::new(Id::new(1), EffectKind::Denoise));
    let path = cache_path(std::path::Path::new("C:/cache"), &project, &sequence, &clip);
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    // 文件名里不出现空格之类的字符：空格换成下划线。
    assert!(name.starts_with("采访_01-"), "{name}");
    assert!(name.ends_with(".cache.mp4"), "{name}");
    assert_eq!(
        path.parent().unwrap(),
        std::path::Path::new("C:/cache"),
        "缓存文件要落在给定目录"
    );
    // 换个参数就换一个文件。
    let mut other = clip.clone();
    other.color = ColorGrade {
        contrast: 10,
        ..ColorGrade::default()
    };
    assert_ne!(
        path,
        cache_path(
            std::path::Path::new("C:/cache"),
            &project,
            &sequence,
            &other
        )
    );
}

#[test]
fn 清理计划先删最旧的() {
    let entries = vec![
        CacheEntry {
            path: std::path::PathBuf::from("a"),
            bytes: 100,
            modified: 30,
        },
        CacheEntry {
            path: std::path::PathBuf::from("b"),
            bytes: 100,
            modified: 10,
        },
        CacheEntry {
            path: std::path::PathBuf::from("c"),
            bytes: 100,
            modified: 20,
        },
    ];
    assert_eq!(total_bytes(&entries), 300);
    // 没超上限：一个都不删。
    assert!(trim_plan(&entries, 300).is_empty());
    // 只差 50：删掉最旧的一个就够了。
    assert_eq!(
        trim_plan(&entries, 250),
        vec![std::path::PathBuf::from("b")]
    );
    // 上限很低：从旧到新一个个删。
    assert_eq!(
        trim_plan(&entries, 50),
        vec![
            std::path::PathBuf::from("b"),
            std::path::PathBuf::from("c"),
            std::path::PathBuf::from("a"),
        ]
    );
}

#[test]
fn 缓存计划走导出那条滤镜链且不带声音() {
    let (project, _asset_path) = project();
    let sequence = sequence();
    let mut clip = clip();
    let plain = cache_plan(&project, &sequence, &clip).expect("干净片段也该能渲染");

    clip.effects
        .push(Effect::new(Id::new(1), EffectKind::Denoise));
    let with_effect = cache_plan(&project, &sequence, &clip).expect("带效果的片段要能渲染");

    // 走的是导出那条路：缩放补边与帧率都在，效果也进了滤镜链。
    assert!(
        with_effect.filter.contains("scale="),
        "{}",
        with_effect.filter
    );
    assert!(
        with_effect.filter.contains("fps="),
        "{}",
        with_effect.filter
    );
    assert_ne!(
        plain.filter, with_effect.filter,
        "效果必须进缓存文件的滤镜链，否则缓存等于原片"
    );
    // 预览另有一路音频，缓存文件里不该再带一条声音。
    assert!(with_effect.audio_label.is_none());
    assert!(
        with_effect.output_args.iter().any(|arg| arg == "-an"),
        "{:?}",
        with_effect.output_args
    );
    assert_eq!(with_effect.inputs, vec![_asset_path]);
}

#[test]
fn 容量文案分档() {
    assert_eq!(size_label(0), "0 MB");
    assert_eq!(size_label(512 * 1024 * 1024), "512 MB");
    assert_eq!(size_label(4 * 1024 * 1024 * 1024), "4.0 GB");
}

#[test]
#[ignore = "需要本机 ffmpeg：cargo test -- --ignored"]
fn 端到端渲染出一份缓存文件() {
    use crate::export::{ExportEvent, ExportTask};
    use futures::StreamExt as _;

    let dir = std::env::temp_dir().join("ngy-cache-e2e");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("建临时目录");
    let source = dir.join("bars.mp4");
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
        ])
        .arg("testsrc=size=320x240:rate=30:duration=2")
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .arg(&source)
        .status()
        .expect("本机要有 ffmpeg");
    assert!(status.success(), "造测试素材失败");

    let mut project = Project::new();
    let asset = crate::media::register(&mut project, &source).expect("登记素材");
    let mut sequence = Sequence::new("序列 1", Rational::FPS_30, 320, 240);
    sequence.tracks.clear();
    let mut clip = ClipItem::new(
        Id::new(1),
        asset,
        Tick::ZERO,
        Tick::from_secs(2.0),
        Tick::ZERO,
    );
    clip.effects
        .push(Effect::new(Id::new(2), EffectKind::Denoise));
    let clip_id = clip.id;
    let mut track = crate::timeline::Track::new(Id::new(3), TrackKind::Video, "V1");
    track.clips.push(clip);
    sequence.tracks.push(track);
    let sequence_id = sequence.id;
    project.sequences = vec![sequence];
    project.active_sequence = sequence_id;
    let sequence = project.sequences[0].clone();
    let clip = sequence.clip(clip_id).expect("片段").clone();

    let plan = cache_plan(&project, &sequence, &clip).expect("生成渲染计划");
    let output = cache_path(&dir.join("cache"), &project, &sequence, &clip);
    std::fs::create_dir_all(output.parent().unwrap()).expect("建缓存目录");

    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    let _task = ExportTask::start(plan, output.clone(), clip.duration(), tx).expect("起 ffmpeg");
    // 端到端测试是手工跑的（`--ignored`），这里就一直等到 ffmpeg 收尾。
    loop {
        let Some(event) = futures::executor::block_on(rx.next()) else {
            panic!("ffmpeg 没给完成事件就断了");
        };
        match event {
            ExportEvent::Finished(path) => {
                assert_eq!(path, output);
                break;
            }
            ExportEvent::Failed(message) => panic!("渲染失败: {message}"),
            ExportEvent::Cancelled => panic!("渲染被取消"),
            ExportEvent::Progress(_) => {}
        }
    }

    let info = crate::media::probe(&output).expect("缓存文件要是能读的视频");
    assert!(
        (info.duration.secs() - 2.0).abs() < 0.2,
        "时长该是 2 秒，实际 {}",
        info.duration.secs()
    );
    // 指纹没变：下一次仍然落在同一个文件上，也就是会命中。
    let again = cache_path(&dir.join("cache"), &project, &sequence, &clip);
    assert_eq!(again, output);

    let _ = std::fs::remove_dir_all(&dir);
}
