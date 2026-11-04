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
