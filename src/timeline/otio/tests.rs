use super::*;
use crate::timeline::{
    ClipItem, Marker, Project, Rational, Sequence, Tick, TrackKind, Transition, TransitionKind,
};

/// 一份能导出的工程：两条素材路径（不必真的存在，`add_asset` 只记绝对路径）。
fn fixture() -> (Project, Sequence) {
    let mut project = Project::new();
    let first = project.add_asset(std::path::Path::new("C:/素材/第一 段.mp4"));
    let second = project.add_asset(std::path::Path::new("C:/素材/second.mp4"));
    let mut sequence = project.sequence().clone();
    sequence.tracks.clear();
    sequence.fps = Rational::FPS_25;
    sequence.width = 3840;
    sequence.height = 2160;

    let video = sequence.add_track(TrackKind::Video);
    let audio = sequence.add_track(TrackKind::Audio);
    // id 先分配好再借轨道：`track_mut` 拿着 `&mut sequence`，中间不能再 `alloc_id`。
    let a_id = sequence.alloc_id();
    let b_id = sequence.alloc_id();
    let transition_id = sequence.alloc_id();
    let audio_clip_id = sequence.alloc_id();

    let a = ClipItem::new(
        a_id,
        first,
        Tick::from_secs(1.0),
        Tick::from_secs(4.0),
        Tick::ZERO,
    )
    .with_name("开场");
    let b = ClipItem::new(
        b_id,
        second,
        Tick::from_secs(2.0),
        Tick::from_secs(6.0),
        Tick::from_secs(5.0),
    );
    let track = sequence.track_mut(video).expect("刚加的轨道");
    track.clips.push(a);
    track.clips.push(b);
    track.transitions.push(Transition::new(
        transition_id,
        a_id,
        b_id,
        TransitionKind::Wipe,
        Tick::from_secs(1.5),
    ));

    let audio_track = sequence.track_mut(audio).expect("刚加的音频轨");
    audio_track.gain = 0.5;
    audio_track.clips.push(ClipItem::new(
        audio_clip_id,
        second,
        Tick::ZERO,
        Tick::from_secs(3.0),
        Tick::from_secs(1.0),
    ));

    sequence.add_marker(Tick::from_secs(2.0), "标记 A");
    (project, sequence)
}

#[test]
fn 导出是标准的_otio_结构() {
    let (project, sequence) = fixture();
    let text = export_sequence(&project, &sequence);
    assert!(text.contains("\"OTIO_SCHEMA\": \"Timeline.1\""), "{text}");
    assert!(text.contains("\"OTIO_SCHEMA\": \"Stack.1\""));
    assert!(text.contains("\"kind\": \"Video\""));
    assert!(text.contains("\"kind\": \"Audio\""));
    // 5 秒处开始的那一段前面有个 5 秒空档。
    assert!(text.contains("\"OTIO_SCHEMA\": \"Gap.1\""));
    // 转场是 OTIO 的 Transition，类型写标准名或自定义名。
    assert!(text.contains("\"OTIO_SCHEMA\": \"Transition.1\""));
    assert!(text.contains("\"transition_type\": \"Custom_Transition\""));
    // 素材路径是 file:// URL，空格转义。
    assert!(text.contains("file:///C:/"), "{text}");
    assert!(text.contains("%20"), "路径里的空格要转义：{text}");
    // 规格与标记在 metadata 里。
    assert!(text.contains("\"fps\": \"25/1\""));
    assert!(text.contains("\"width\": 3840"));
    assert!(text.contains("标记 A"));
}

#[test]
fn 往返不丢片段位置入出点与转场() {
    let (project, sequence) = fixture();
    let text = export_sequence(&project, &sequence);
    let imported = import_timeline(&text).expect("自己导出的必须能读回来");

    assert_eq!(imported.name, sequence.name);
    assert_eq!(imported.fps, Some(Rational::FPS_25));
    assert_eq!(imported.width, Some(3840));
    assert_eq!(imported.height, Some(2160));
    assert_eq!(imported.markers.len(), 1);
    assert_eq!(imported.markers[0].0, Tick::from_secs(2.0));

    assert_eq!(imported.tracks.len(), 2);
    let video = &imported.tracks[0];
    assert_eq!(video.kind, TrackKind::Video);
    assert_eq!(video.clips.len(), 2);
    assert_eq!(video.clips[0].start, Tick::ZERO);
    assert_eq!(video.clips[0].source_in, Tick::from_secs(1.0));
    assert_eq!(video.clips[0].source_out, Tick::from_secs(4.0));
    assert_eq!(video.clips[0].name, "开场");
    // 第二段前面是空档，位置原样。
    assert_eq!(video.clips[1].start, Tick::from_secs(5.0));
    assert_eq!(video.clips[1].source_in, Tick::from_secs(2.0));
    assert_eq!(video.clips[1].source_out, Tick::from_secs(6.0));

    assert_eq!(video.transitions.len(), 1);
    assert_eq!(video.transitions[0].after, 1);
    assert_eq!(video.transitions[0].kind, TransitionKind::Wipe);
    assert_eq!(video.transitions[0].duration, Tick::from_secs(1.5));

    let audio = &imported.tracks[1];
    assert_eq!(audio.kind, TrackKind::Audio);
    assert!((audio.gain - 0.5).abs() < 1e-6, "轨道音量要留住");
    assert_eq!(audio.clips.len(), 1);
    assert_eq!(audio.clips[0].start, Tick::from_secs(1.0));
    assert!(imported.warnings.is_empty(), "{:?}", imported.warnings);
}

#[test]
fn 对方软件写的结构也能导入() {
    // 典型的手写 / Resolve 导出：没有 metadata.ngy，转场是 SMPTE_Dissolve。
    let text = r#"{
      "OTIO_SCHEMA": "Timeline.1",
      "name": "Resolve 时间线",
      "tracks": {
        "OTIO_SCHEMA": "Stack.1",
        "children": [{
          "OTIO_SCHEMA": "Track.1",
          "name": "V1",
          "kind": "Video",
          "children": [
            {"OTIO_SCHEMA": "Clip.1", "name": "a",
             "source_range": {"OTIO_SCHEMA": "TimeRange.1",
               "start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 0},
               "duration": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 48}},
             "media_reference": {"OTIO_SCHEMA": "ExternalReference.1",
               "target_url": "file:///D:/media/a.mp4"}},
            {"OTIO_SCHEMA": "Transition.1", "name": "dissolve",
             "transition_type": "SMPTE_Dissolve",
             "in_offset": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 6},
             "out_offset": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 6}},
            {"OTIO_SCHEMA": "Gap.1",
             "source_range": {"OTIO_SCHEMA": "TimeRange.1",
               "start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 0},
               "duration": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 24}}},
            {"OTIO_SCHEMA": "Clip.1", "name": "b",
             "source_range": {"OTIO_SCHEMA": "TimeRange.1",
               "start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 12},
               "duration": {"OTIO_SCHEMA": "RationalTime.1", "rate": 24, "value": 24}},
             "media_reference": {"OTIO_SCHEMA": "ExternalReference.1",
               "target_url": "file:///D:/media/b.mp4"}}
          ]
        }]
      }
    }"#;
    let imported = import_timeline(text).expect("标准结构必须能读");
    assert_eq!(imported.name, "Resolve 时间线");
    assert_eq!(imported.fps, None, "没写 metadata 就没有帧率，交给界面沿用");
    let track = &imported.tracks[0];
    assert_eq!(track.clips.len(), 2);
    // 48 帧 @24fps = 2 秒；空档 24 帧 = 1 秒。
    assert_eq!(track.clips[0].start, Tick::ZERO);
    assert_eq!(track.clips[0].source_out, Tick::from_secs(2.0));
    assert_eq!(track.clips[1].start, Tick::from_secs(3.0));
    assert_eq!(track.clips[1].source_in, Tick::from_secs(0.5));
    // 转场时长 = in_offset + out_offset = 12 帧 = 0.5 秒。
    assert_eq!(track.transitions.len(), 1);
    assert_eq!(track.transitions[0].after, 1);
    assert_eq!(track.transitions[0].kind, TransitionKind::CrossDissolve);
    assert_eq!(track.transitions[0].duration, Tick::from_secs(0.5));
}

#[test]
fn 不认识的转场与效果降级成提示() {
    let (project, mut sequence) = fixture();
    // 手工塞一条认不出来的效果，模拟对方软件的特有参数。
    let video = sequence.tracks[0].id;
    sequence.track_mut(video).expect("视频轨").clips[0].effects =
        vec![crate::timeline::Effect::new(
            crate::timeline::Id::new(99),
            crate::timeline::EffectKind::Unknown,
        )];
    let text = export_sequence(&project, &sequence);
    let imported = import_timeline(&text).expect("坏东西不该让整份文件读不出来");
    assert!(
        imported
            .warnings
            .iter()
            .any(|line| line.contains("unknown")),
        "认不出的效果要点名：{:?}",
        imported.warnings
    );
    assert!(
        imported.tracks[0].clips[0].effects.is_empty(),
        "认不出的效果要被丢掉"
    );
}

#[test]
fn 变速片段按倍率换算后续片段的位置() {
    let text = r#"{
      "OTIO_SCHEMA": "Timeline.1",
      "name": "变速",
      "tracks": {
        "OTIO_SCHEMA": "Stack.1",
        "children": [{
          "OTIO_SCHEMA": "Track.1", "kind": "Video", "name": "V1",
          "children": [
            {"OTIO_SCHEMA": "Clip.1", "name": "快的",
             "source_range": {"OTIO_SCHEMA": "TimeRange.1",
               "start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 30, "value": 0},
               "duration": {"OTIO_SCHEMA": "RationalTime.1", "rate": 30, "value": 120}},
             "media_reference": {"OTIO_SCHEMA": "ExternalReference.1",
               "target_url": "file:///D:/media/a.mp4"},
             "effects": [{"OTIO_SCHEMA": "LinearTimeWarp.1", "effect_name": "speed",
                          "time_scalar": 2.0}]},
            {"OTIO_SCHEMA": "Clip.1", "name": "后面的",
             "source_range": {"OTIO_SCHEMA": "TimeRange.1",
               "start_time": {"OTIO_SCHEMA": "RationalTime.1", "rate": 30, "value": 0},
               "duration": {"OTIO_SCHEMA": "RationalTime.1", "rate": 30, "value": 30}},
             "media_reference": {"OTIO_SCHEMA": "ExternalReference.1",
               "target_url": "file:///D:/media/b.mp4"}}
          ]
        }]
      }
    }"#;
    let imported = import_timeline(text).expect("变速片段要读得出来");
    let track = &imported.tracks[0];
    assert!((track.clips[0].speed - 2.0).abs() < 1e-6);
    // 4 秒素材 2 倍速 = 时间线上 2 秒，第二段紧跟着。
    assert_eq!(track.clips[1].start, Tick::from_secs(2.0));
}

#[test]
fn 不是时间线就明确报错() {
    let error = import_timeline(r#"{"OTIO_SCHEMA": "Clip.1"}"#).expect_err("Clip 不是时间线");
    assert!(matches!(error, OtioError::NotTimeline(_)));
    assert!(error.to_string().contains("Clip.1"), "{error}");

    let error = import_timeline("不是 JSON").expect_err("乱码要报 JSON 错");
    assert!(matches!(error, OtioError::Json(_)));

    let error = import_timeline(r#"{"OTIO_SCHEMA": "Timeline.1"}"#).expect_err("一条轨道都没有");
    assert_eq!(error, OtioError::NoTracks);
}

#[test]
fn 文件_url_往返中文与空格() {
    let path = std::path::Path::new("C:/剪辑 素材/第一 段#1.mp4");
    let url = file_url(path);
    assert!(url.starts_with("file:///C:/"), "{url}");
    assert!(url.contains("%20"), "{url}");
    assert!(url.contains("%23"), "井号也要转义：{url}");
    assert_eq!(path_from_url(&url).as_deref(), Some(path));
    // 非 file 协议不认。
    assert!(path_from_url("https://example.com/a.mp4").is_none());
}

#[test]
fn 字幕轨进元数据也能回来() {
    let (project, mut sequence) = fixture();
    let subtitle_track = sequence.add_track(TrackKind::Subtitle);
    let id = sequence.alloc_id();
    sequence
        .track_mut(subtitle_track)
        .expect("刚加的字幕轨")
        .subtitles
        .push(crate::timeline::SubtitleItem::new(
            id,
            Tick::from_secs(1.0),
            Tick::from_secs(2.0),
            "你好",
        ));

    let text = export_sequence(&project, &sequence);
    // 字幕不进结构：不能凭空多出一条空 Video 轨。
    assert_eq!(
        text.matches("\"kind\": \"Video\"").count(),
        1,
        "字幕轨不该在 OTIO 结构里多出一条空 Video 轨"
    );
    let imported = import_timeline(&text).expect("读回来");
    let subtitle = imported
        .tracks
        .iter()
        .find(|track| track.kind == TrackKind::Subtitle)
        .expect("字幕轨要能回来");
    assert_eq!(subtitle.subtitles.len(), 1);
    assert_eq!(subtitle.subtitles[0].text, "你好");
    assert_eq!(subtitle.subtitles[0].start, Tick::from_secs(1.0));
}

#[test]
fn 标记随_metadata_往返() {
    let (project, mut sequence) = fixture();
    sequence.markers.clear();
    let marker_id = sequence.alloc_id();
    sequence
        .markers
        .push(Marker::new(marker_id, Tick::from_secs(7.5), "第二幕"));
    let imported = import_timeline(&export_sequence(&project, &sequence)).expect("读回来");
    assert_eq!(
        imported.markers,
        vec![(Tick::from_secs(7.5), "第二幕".to_string())]
    );
}
