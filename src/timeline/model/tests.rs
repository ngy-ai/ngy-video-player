//! 序列模型的往返、id 稳定性与不变量检查。

use super::*;

fn sample() -> Sequence {
    let mut sequence = Sequence::new("测试序列", Rational::FPS_29_97, 3840, 2160);
    let video = sequence.tracks[0].id;
    let audio = sequence.tracks[1].id;

    let first = sequence.alloc_id();
    let clip = ClipItem::new(
        first,
        AssetId::new(7),
        Tick::from_micros(1_000_000),
        Tick::from_micros(6_000_000),
        Tick::ZERO,
    )
    .with_name("开场");
    sequence.track_mut(video).unwrap().clips.push(clip);

    let second = sequence.alloc_id();
    sequence.track_mut(audio).unwrap().clips.push(ClipItem::new(
        second,
        AssetId::new(7),
        Tick::from_micros(1_000_000),
        Tick::from_micros(6_000_000),
        Tick::ZERO,
    ));

    let before = first;
    let after = sequence.alloc_id();
    sequence.track_mut(video).unwrap().clips.push(ClipItem::new(
        after,
        AssetId::new(8),
        Tick::ZERO,
        Tick::from_micros(2_000_000),
        Tick::from_micros(5_000_000),
    ));
    let transition = sequence.alloc_id();
    sequence
        .track_mut(video)
        .unwrap()
        .transitions
        .push(Transition::new(
            transition,
            before,
            after,
            TransitionKind::CrossDissolve,
            Tick::from_millis(500),
        ));

    sequence.add_marker(Tick::from_secs(2.0), "第一节");
    sequence
}

#[test]
fn 空序列可以序列化往返() {
    let sequence = Sequence::default();
    let text = serde_json::to_string(&sequence).expect("序列化");
    let back: Sequence = serde_json::from_str(&text).expect("反序列化");
    assert_eq!(back, sequence);
    assert!(back.tracks.is_empty());
    assert_eq!(back.duration(), Tick::ZERO);
}

#[test]
fn 序列往返保留轨道片段转场与标记() {
    let sequence = sample();
    let text = serde_json::to_string(&sequence).expect("序列化");
    let back: Sequence = serde_json::from_str(&text).expect("反序列化");

    assert_eq!(back, sequence, "整条序列原样回来");
    assert_eq!(back.fps, Rational::FPS_29_97);
    assert_eq!((back.width, back.height), (3840, 2160));
    assert_eq!(back.tracks.len(), 2);
    assert_eq!(back.tracks[0].clips[0].name, "开场");
    assert_eq!(back.tracks[0].transitions.len(), 1);
    assert_eq!(back.markers[0].name, "第一节");
    assert!(back.validate().is_ok());

    // 帧率与时间在文件里都是可读的整数/分数，不是浮点。
    assert!(text.contains("\"30000/1001\""), "{text}");
    assert!(text.contains("\"source_in\":1000000"), "时间是整数微秒");
}

#[test]
fn 同一位置再按一次标记等于删除() {
    let mut sequence = Sequence::new("标记", Rational::FPS_25, 1920, 1080);
    let at = Tick::from_secs(2.0);

    assert!(
        sequence.toggle_marker(at, "标记 1", Tick::ZERO),
        "第一次是新增"
    );
    assert_eq!(sequence.markers.len(), 1);
    assert!(
        !sequence.toggle_marker(at, "标记 2", Tick::ZERO),
        "同一位置再按一次是删除"
    );
    assert!(sequence.markers.is_empty());

    // 差一秒就是另一个位置，该加新的。
    assert!(sequence.toggle_marker(at, "A", Tick::ZERO));
    assert!(sequence.toggle_marker(Tick::from_secs(3.0), "B", Tick::ZERO));
    assert_eq!(sequence.markers.len(), 2);
    assert_eq!(sequence.markers[0].name, "A", "标记按时间排序");
    assert!(sequence.validate().is_ok());
}

#[test]
fn 标记的命中范围按像素容差算() {
    let mut sequence = Sequence::new("标记", Rational::FPS_25, 1920, 1080);
    // 8 像素/秒的默认缩放下，一个像素是 125 毫秒。
    let tolerance = Tick::from_millis(125);
    let at = Tick::from_secs(2.0);

    assert!(sequence.toggle_marker(at, "标记 1", tolerance));
    // 播放头拖回来只落在同一像素里：差 60 毫秒也算回到原位，删掉它。
    assert!(
        !sequence.toggle_marker(Tick::from_millis(2060), "标记 2", tolerance),
        "同像素内的偏差不该再叠一个新标记"
    );
    assert!(sequence.markers.is_empty());

    // 容差之外就是另一个位置：两枚都留着。
    assert!(sequence.toggle_marker(at, "A", tolerance));
    assert!(sequence.toggle_marker(Tick::from_secs(2.2), "B", tolerance));
    assert_eq!(sequence.markers.len(), 2);

    // 两枚都落在容差里时，删最近的那一枚。
    assert!(!sequence.toggle_marker(Tick::from_micros(2_190_000), "C", tolerance));
    assert_eq!(
        sequence.markers.iter().map(|m| m.at).collect::<Vec<_>>(),
        vec![at],
        "远处那枚必须还在"
    );
}

#[test]
fn 轨道状态随序列往返且老工程默认正常() {
    let mut sequence = sample();
    sequence.tracks[0].muted = true;
    sequence.tracks[1].locked = true;

    let text = serde_json::to_string(&sequence).expect("序列化");
    let back: Sequence = serde_json::from_str(&text).expect("反序列化");
    assert!(back.tracks[0].muted, "静音状态要存下来");
    assert!(back.tracks[1].locked, "锁定状态要存下来");
    assert!(!back.tracks[0].locked);

    // 老工程里没有这两个字段：读出来必须是「正常可编辑」，
    // 不能因为 `bool` 的默认值变成「静音 + 锁定」。
    let legacy =
        r#"{"name":"旧工程","fps":"25/1","tracks":[{"id":1,"kind":"video","name":"视频"}]}"#;
    let back: Sequence = serde_json::from_str(legacy).expect("老工程");
    assert!(!back.tracks[0].muted);
    assert!(!back.tracks[0].locked);
}

#[test]
fn 轨道音量与独奏随序列往返且老工程默认原样() {
    let mut sequence = sample();
    sequence.tracks[0].gain = 0.25;
    sequence.tracks[1].solo = true;

    let text = serde_json::to_string(&sequence).expect("序列化");
    let back: Sequence = serde_json::from_str(&text).expect("反序列化");
    assert_eq!(back.tracks[0].gain, 0.25);
    assert!(back.tracks[1].solo);

    // 老工程没有这两个键：音量必须是 1.0（`f32` 的默认 0.0 会把整条轨变哑）。
    let legacy =
        r#"{"name":"旧工程","fps":"25/1","tracks":[{"id":1,"kind":"audio","name":"音频"}]}"#;
    let back: Sequence = serde_json::from_str(legacy).expect("老工程");
    assert_eq!(back.tracks[0].gain, 1.0, "缺字段按原样，不是静音");
    assert!(!back.tracks[0].solo);
    assert_eq!(back.tracks[0].gain_label(), "0 dB");
}

#[test]
fn 音量文案按分贝显示() {
    let mut sequence = sample();
    let track = &mut sequence.tracks[1];
    assert_eq!(track.gain_label(), "0 dB", "没改过就是原样");

    track.gain = 0.5;
    assert_eq!(track.gain_label(), "-6.0 dB");
    track.gain = 2.0;
    assert_eq!(track.gain_label(), "+6.0 dB");
    track.gain = 0.0;
    assert_eq!(track.gain_label(), "静音");
    // 独奏只看音频轨：视频轨上勾了也不会把别的音频轨挤掉。
    sequence.tracks[0].solo = true;
    assert!(!sequence.audio_solo());
    sequence.tracks[1].solo = true;
    assert!(sequence.audio_solo());
}

#[test]
fn 一千个片段往返不丢数据() {
    let mut sequence = Sequence::new("长片", Rational::FPS_25, 1920, 1080);
    let video = sequence.tracks[0].id;
    for index in 0..1000i64 {
        let id = sequence.alloc_id();
        let start = Tick::from_micros(index * 1_000_000);
        sequence.track_mut(video).unwrap().clips.push(ClipItem::new(
            id,
            AssetId::new((index % 13) as u64 + 1),
            Tick::ZERO,
            Tick::from_micros(1_000_000),
            start,
        ));
    }

    assert_eq!(sequence.duration(), Tick::from_micros(1_000_000_000));
    assert_eq!(sequence.clips().count(), 1000);
    assert!(sequence.validate().is_ok());

    let text = serde_json::to_string(&sequence).expect("序列化");
    let back: Sequence = serde_json::from_str(&text).expect("反序列化");
    assert_eq!(back.tracks[0].clips.len(), 1000);
    assert_eq!(back.duration(), sequence.duration());
    assert!(back.validate().is_ok());
}

#[test]
fn 万级片段只取视口里那几段() {
    let mut sequence = Sequence::new("长片", Rational::FPS_25, 1920, 1080);
    let video = sequence.tracks[0].id;
    // 每条 1 秒、首尾相接：第 i 条占 [i, i+1)。
    for index in 0..10_000i64 {
        let id = sequence.alloc_id();
        sequence.track_mut(video).unwrap().clips.push(ClipItem::new(
            id,
            AssetId::new(1),
            Tick::ZERO,
            Tick::from_secs(1.0),
            Tick::from_micros(index * 1_000_000),
        ));
    }

    let track = &sequence.tracks[0];
    let window: Vec<_> = track
        .visible_clips(Tick::from_secs(5000.5), Tick::from_secs(5002.5))
        .map(|clip| clip.start)
        .collect();
    assert_eq!(
        window,
        vec![
            Tick::from_secs(5000.0),
            Tick::from_secs(5001.0),
            Tick::from_secs(5002.0)
        ],
        "跨进视口的那段也算，正好在右边界的不算"
    );

    // 边界：视口落在最后一条之后，什么都不给。
    assert_eq!(
        track
            .visible_clips(Tick::from_secs(20_000.0), Tick::from_secs(20_010.0))
            .count(),
        0
    );
}

#[test]
fn 片段在素材时间与序列时间之间换算() {
    let clip = ClipItem::new(
        Id::new(1),
        AssetId::new(1),
        Tick::from_micros(10_000_000),
        Tick::from_micros(15_000_000),
        Tick::from_micros(60_000_000),
    );

    assert_eq!(clip.duration(), Tick::from_micros(5_000_000));
    assert_eq!(clip.end(), Tick::from_micros(65_000_000));
    assert!(clip.covers(Tick::from_micros(60_000_000)));
    assert!(!clip.covers(Tick::from_micros(65_000_000)), "出点不含");
    assert_eq!(
        clip.source_at(Tick::from_micros(62_000_000)),
        Some(Tick::from_micros(12_000_000))
    );
    assert_eq!(
        clip.timeline_at(Tick::from_micros(12_000_000)),
        Some(Tick::from_micros(62_000_000))
    );
    assert_eq!(
        clip.source_at(Tick::from_micros(1)),
        None,
        "片段外没有对应时间"
    );
    assert_eq!(clip.timeline_at(Tick::from_micros(15_000_000)), None);
}

/// 变速改的是片段在时间线上的长度，素材跨度不动。
#[test]
fn 倍率压缩片段在时间线上的长度() {
    let mut clip = ClipItem::new(
        Id::new(1),
        AssetId::new(1),
        Tick::ZERO,
        Tick::from_micros(4_000_000),
        Tick::from_micros(10_000_000),
    );
    assert_eq!(clip.span(), Tick::from_micros(4_000_000));
    assert_eq!(clip.duration(), Tick::from_micros(4_000_000));

    clip.speed = 2.0;
    assert_eq!(clip.motion(), Tick::from_micros(2_000_000));
    assert_eq!(clip.end(), Tick::from_micros(12_000_000));
    // 序列上 1 秒对应素材 2 秒。
    assert_eq!(
        clip.source_at(Tick::from_micros(11_000_000)),
        Some(Tick::from_micros(2_000_000))
    );
    assert_eq!(
        clip.timeline_at(Tick::from_micros(2_000_000)),
        Some(Tick::from_micros(11_000_000))
    );

    clip.speed = 0.5;
    assert_eq!(clip.duration(), Tick::from_micros(8_000_000));
}

/// 片尾定格：视频多停 `hold`，那段里取到的永远是最后一帧。
#[test]
fn 定格把时长拉长且落在最后一帧() {
    let mut clip = ClipItem::new(
        Id::new(1),
        AssetId::new(1),
        Tick::ZERO,
        Tick::from_micros(2_000_000),
        Tick::ZERO,
    );
    clip.hold = Tick::from_micros(3_000_000);
    assert_eq!(clip.motion(), Tick::from_micros(2_000_000));
    assert_eq!(clip.duration(), Tick::from_micros(5_000_000));
    assert_eq!(
        clip.source_at(Tick::from_micros(4_000_000)),
        Some(Tick::from_micros(1_999_999)),
        "定格段重复最后一帧"
    );
}

/// 坏倍率（0、负、NaN）按原速算：一条坏数据不该让片段变成 0 长。
#[test]
fn 坏倍率退回原速() {
    let mut clip = ClipItem::new(
        Id::new(1),
        AssetId::new(1),
        Tick::ZERO,
        Tick::from_micros(2_000_000),
        Tick::ZERO,
    );
    for bad in [0.0, -1.0, f32::NAN] {
        clip.speed = bad;
        assert_eq!(clip.rate(), 1.0);
        assert_eq!(clip.duration(), Tick::from_micros(2_000_000));
    }
    clip.speed = 100.0;
    assert_eq!(clip.rate(), ClipItem::MAX_SPEED);
}

#[test]
fn 编号由序列统一分配且存盘后继续增长() {
    let sequence = sample();
    let used: Vec<Id> = sequence
        .clips()
        .map(|(_, clip)| clip.id)
        .chain(sequence.tracks.iter().map(|track| track.id))
        .chain(sequence.markers.iter().map(|marker| marker.id))
        .collect();
    let unique: HashSet<Id> = used.iter().copied().collect();
    assert_eq!(unique.len(), used.len(), "同一个序列里 id 不重复");

    let text = serde_json::to_string(&sequence).expect("序列化");
    let mut back: Sequence = serde_json::from_str(&text).expect("反序列化");
    let fresh = back.alloc_id();
    assert!(
        !sequence.id_is_used(fresh),
        "重开后新 id 不能撞上旧实体：{fresh}"
    );
}

#[test]
fn 新序列带视频轨与音频轨() {
    let sequence = Sequence::new("新工程", Rational::FPS_25, 1280, 720);
    assert_eq!(sequence.tracks.len(), 2);
    assert_eq!(sequence.tracks[0].kind, TrackKind::Video);
    assert_eq!(sequence.tracks[0].name, "视频 1");
    assert_eq!(sequence.tracks[1].kind, TrackKind::Audio);
    assert_eq!(sequence.tracks[1].name, "音频 1");
    assert_eq!(sequence.fps, Rational::FPS_25);
    assert_eq!(sequence.duration(), Tick::ZERO);

    let mut sequence = sequence;
    let video = sequence.add_track(TrackKind::Video);
    assert_eq!(sequence.track(video).unwrap().name, "视频 2");
    assert!(sequence.id_is_used(video));
}

#[test]
fn 轨道终点取最后一个片段的出点() {
    let mut track = Track::new(Id::new(1), TrackKind::Video, "视频 1");
    assert_eq!(track.end(), Tick::ZERO);

    track.clips.push(ClipItem::new(
        Id::new(2),
        AssetId::new(1),
        Tick::ZERO,
        Tick::from_micros(2_000_000),
        Tick::from_micros(8_000_000),
    ));
    track.clips.push(ClipItem::new(
        Id::new(3),
        AssetId::new(1),
        Tick::ZERO,
        Tick::from_micros(1_000_000),
        Tick::from_micros(1_000_000),
    ));
    assert_eq!(track.end(), Tick::from_micros(10_000_000));
    track.sort_clips();
    assert_eq!(track.clips[0].id, Id::new(3), "按起点排序");
}

#[test]
fn 标记按时间排序() {
    let mut sequence = Sequence::default();
    sequence.add_marker(Tick::from_secs(5.0), "后");
    sequence.add_marker(Tick::from_secs(1.0), "前");

    assert_eq!(sequence.markers[0].name, "前");
    assert_eq!(sequence.markers[1].name, "后");
    assert!(sequence.validate().is_ok());
}

#[test]
fn 校验能发现重叠与非法时长() {
    let mut sequence = Sequence::default();
    let track_id = sequence.add_track(TrackKind::Video);
    let first = sequence.alloc_id();
    let second = sequence.alloc_id();
    let track = sequence.track_mut(track_id).unwrap();
    track.clips.push(ClipItem::new(
        first,
        AssetId::new(1),
        Tick::ZERO,
        Tick::from_micros(2_000_000),
        Tick::ZERO,
    ));
    track.clips.push(ClipItem::new(
        second,
        AssetId::new(1),
        Tick::ZERO,
        Tick::from_micros(2_000_000),
        Tick::from_micros(1_000_000),
    ));
    assert_eq!(
        sequence.validate(),
        Err(ModelError::Overlap(track_id)),
        "两个片段压在一起"
    );

    // 首尾正好相接是合法的。
    sequence.track_mut(track_id).unwrap().clips[1].start = Tick::from_micros(2_000_000);
    assert!(sequence.validate().is_ok());

    // 出点不晚于入点 = 空片段。
    sequence.track_mut(track_id).unwrap().clips[1].source_out = Tick::ZERO;
    sequence.track_mut(track_id).unwrap().clips[1].source_in = Tick::ZERO;
    assert_eq!(sequence.validate(), Err(ModelError::EmptyClip(second)));
}

#[test]
fn 校验能发现重复_id_与悬空转场() {
    let mut sequence = Sequence::default();
    let track_id = sequence.add_track(TrackKind::Video);
    let clip_id = sequence.alloc_id();
    let transition_id = sequence.alloc_id();
    sequence
        .track_mut(track_id)
        .unwrap()
        .clips
        .push(ClipItem::new(
            clip_id,
            AssetId::new(1),
            Tick::ZERO,
            Tick::from_micros(1_000_000),
            Tick::ZERO,
        ));
    sequence
        .track_mut(track_id)
        .unwrap()
        .transitions
        .push(Transition::new(
            transition_id,
            clip_id,
            Id::new(999),
            TransitionKind::Fade,
            Tick::from_millis(200),
        ));
    assert_eq!(
        sequence.validate(),
        Err(ModelError::DanglingTransition(transition_id)),
        "转场指向不存在的片段"
    );

    sequence.tracks[0].transitions.clear();
    sequence.tracks[0].clips[0].id = track_id;
    assert_eq!(sequence.validate(), Err(ModelError::DuplicateId(track_id)));
}

#[test]
fn 修好下一个_id_之后不再撞车() {
    // 手改过的工程：片段用了 100，计数器还停在 1。
    let mut sequence = Sequence::default();
    let track_id = sequence.add_track(TrackKind::Video);
    sequence.tracks[0].clips.push(ClipItem::new(
        Id::new(100),
        AssetId::new(1),
        Tick::ZERO,
        Tick::from_micros(1_000_000),
        Tick::ZERO,
    ));
    sequence.repair_next_id();

    let fresh = sequence.alloc_id();
    assert_ne!(fresh, Id::new(100));
    assert!(fresh.raw() > 100);
    assert!(sequence.track(track_id).is_some());
}
