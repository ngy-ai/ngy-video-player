//! 字幕模型与 ASS 文本的单测：这里钉住的是「时间线与成片看到的字幕是同一条」。

use super::*;
use crate::timeline::{Rational, Sequence, TrackKind};

fn sequence_with(items: Vec<SubtitleItem>) -> Sequence {
    let mut sequence = Sequence::new("字幕测试", Rational::FPS_30, 1920, 1080);
    let track = sequence.add_track(TrackKind::Subtitle);
    for item in items {
        sequence.track_mut(track).unwrap().subtitles.push(item);
    }
    sequence
}

fn item(start: f64, end: f64, text: &str) -> SubtitleItem {
    SubtitleItem::new(
        Id::new(1000),
        Tick::from_secs(start),
        Tick::from_secs(end),
        text,
    )
}

#[test]
fn 默认样式与坏值规整() {
    let style = SubtitleStyle::default();
    assert_eq!(style.font, FONTS[0]);
    assert_eq!(style.size, 48);
    assert_eq!(style.position, SubtitlePosition::Bottom);
    assert!(COLORS.iter().any(|(rgb, _)| *rgb == style.color));

    let mut bad = SubtitleStyle {
        font: "  ".to_string(),
        size: 999,
        color: 0xff12_3456,
        outline: 40,
        position: SubtitlePosition::Top,
    };
    bad.normalize();
    assert_eq!(bad.font, FONTS[0]);
    assert_eq!(bad.size, MAX_SIZE);
    assert_eq!(bad.outline, MAX_OUTLINE);
    assert_eq!(bad.color, 0x12_3456, "高字节丢掉，只留 RGB");
}

#[test]
fn 时间规整与区间判定() {
    let mut entry = SubtitleItem::new(Id::new(7), Tick::from_secs(-1.0), Tick::ZERO, "  \r\n  ");
    entry.normalize();
    assert_eq!(entry.start, Tick::ZERO);
    assert_eq!(entry.end, Tick::from_secs(3.0));
    assert_eq!(entry.text, "新字幕");
    assert!(entry.covers(Tick::ZERO));
    assert!(!entry.covers(Tick::from_secs(3.0)), "出点不含");
    assert_eq!(entry.duration(), Tick::from_secs(3.0));
}

#[test]
fn 没有字幕或者轨道全隐藏时不生成脚本() {
    let empty = sequence_with(vec![]);
    assert!(ass_script(&empty, None).is_none());

    let mut sequence = sequence_with(vec![item(1.0, 2.0, "你好")]);
    sequence.tracks.last_mut().unwrap().muted = true;
    assert!(ass_script(&sequence, None).is_none());
    assert!(at(&sequence, Tick::from_secs(1.5)).is_none());
}

#[test]
fn 脚本带序列尺寸与时间样式() {
    let sequence = sequence_with(vec![item(5.0, 8.25, "第一句")]);
    let script = ass_script(&sequence, None).unwrap();
    assert!(script.contains("PlayResX: 1920"));
    assert!(script.contains("PlayResY: 1080"));
    assert!(script.contains("[V4+ Styles]"));
    assert!(script.contains("[Events]"));
    assert!(
        script.contains("Dialogue: 0,0:00:05.00,0:00:08.25,S0,,0,0,0,,第一句"),
        "{script}"
    );
    // 白色 RRGGBB 在 ASS 里是 BGR，描边 2px，底部 alignment=2。
    assert!(script.contains("Style: S0,Arial,48,&H00FFFFFF"), "{script}");
    assert!(
        script.contains(",1,2,1,2,20,20,20,1"),
        "描边/阴影/位置：{script}"
    );
}

#[test]
fn 三档位置与颜色都写进样式行() {
    for (position, alignment) in [
        (SubtitlePosition::Top, 8),
        (SubtitlePosition::Middle, 5),
        (SubtitlePosition::Bottom, 2),
    ] {
        let mut entry = item(0.0, 1.0, "位置");
        entry.style.position = position;
        entry.style.color = 0x12_34_56;
        let script = ass_script(&sequence_with(vec![entry]), None).unwrap();
        assert!(script.contains("&H00563412&"), "{script}");
        assert!(
            script.contains(&format!(",1,2,1,{alignment},20,20,20,1")),
            "{script}"
        );
    }
}

#[test]
fn 文本换行与裸大括号转义() {
    let script = ass_script(
        &sequence_with(vec![item(0.0, 1.0, "第一行\n第二行 {粗体}")]),
        None,
    )
    .unwrap();
    assert!(script.contains(r"第一行\N第二行 (粗体)"), "{script}");
    assert!(!script.contains('{'), "大括号不能留给 libass 解析");
}

#[test]
fn 相同样式共用一行不同样式各一行() {
    let mut second = item(1.0, 2.0, "第二句");
    second.style.size = 24;
    let script = ass_script(&sequence_with(vec![item(0.0, 1.0, "第一句"), second]), None).unwrap();
    assert!(script.contains("Style: S0,Arial,48"));
    assert!(script.contains("Style: S1,Arial,24"));
    assert!(script.contains(",S0,,0,0,0,,第一句"));
    assert!(script.contains(",S1,,0,0,0,,第二句"));
    assert_eq!(script.matches("Dialogue: ").count(), 2);
}

#[test]
fn 工作区把字幕裁断并从零起算() {
    let sequence = sequence_with(vec![
        item(0.0, 2.0, "切掉"),
        item(4.0, 8.0, "跨过入点"),
        item(6.0, 9.0, "整段"),
        item(11.0, 13.0, "切掉尾部"),
    ]);
    let script = ass_script(
        &sequence,
        Some((Tick::from_secs(5.0), Tick::from_secs(10.0))),
    )
    .unwrap();
    assert!(!script.contains("切掉"), "整段落在区间外：{script}");
    assert!(script.contains("Dialogue: 0,0:00:00.00,0:00:03.00,S0,,0,0,0,,跨过入点"));
    assert!(script.contains("Dialogue: 0,0:00:01.00,0:00:04.00,S0,,0,0,0,,整段"));
    assert!(!script.contains("切掉尾部"), "区间外不写空条目");
}

#[test]
fn 时间戳四舍五入到厘秒且小时进位() {
    let sequence = sequence_with(vec![item(3599.996, 3661.234, "长片")]);
    let script = ass_script(&sequence, Some((Tick::ZERO, Tick::from_secs(3700.0)))).unwrap();
    assert!(script.contains("1:00:00.00,1:01:01.23"), "{script}");
}

#[test]
fn 预览取靠上的未隐藏轨与同一时刻第一条() {
    let mut sequence = sequence_with(vec![item(1.0, 3.0, "下面")]);
    let top = sequence.add_track(TrackKind::Subtitle);
    sequence
        .track_mut(top)
        .unwrap()
        .subtitles
        .push(item(1.0, 3.0, "上面"));
    // 轨道列表里后加的在下面，先找的是第一条轨。
    assert_eq!(at(&sequence, Tick::from_secs(2.0)).unwrap().text, "下面");
    assert!(at(&sequence, Tick::from_secs(3.0)).is_none());

    let first = sequence
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Subtitle)
        .unwrap()
        .id;
    sequence.track_mut(first).unwrap().muted = true;
    assert_eq!(at(&sequence, Tick::from_secs(2.0)).unwrap().text, "上面");
}
