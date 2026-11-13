//! SRT / VTT 解析与生成的单测：往返、时间码变体、行内标签、坏块跳过。

use super::*;

fn s(start: f64, end: f64, text: &str) -> Cue {
    Cue {
        start: Tick::from_secs(start),
        end: Tick::from_secs(end),
        text: text.to_string(),
    }
}

#[test]
fn srt_往返不丢文字与时间() {
    let cues = vec![s(1.0, 3.5, "第一句"), s(4.0, 6.25, "第二句\n换行")];
    let text = format_srt(&cues);
    assert!(text.contains("00:00:01,000 --> 00:00:03,500"));
    assert!(text.contains("00:00:06,250"));
    assert!(text.contains("第二句\r\n换行"), "内部换行保留");
    let back = parse(&text);
    assert_eq!(back.len(), 2);
    assert_eq!(back[0].start, Tick::from_millis(1000));
    assert_eq!(back[0].end, Tick::from_millis(3500));
    assert_eq!(back[1].text, "第二句\n换行");
}

#[test]
fn vtt_头与点号时间码() {
    let cues = vec![s(0.0, 2.0, "hello")];
    let text = format_vtt(&cues);
    assert!(text.starts_with("WEBVTT\n\n"));
    assert!(text.contains("00:00:00.000 --> 00:00:02.000"));
    let back = parse(&text);
    assert_eq!(back, cues);
}

#[test]
fn 省略小时与逗号点号混写都能读() {
    let text = "1\n00:01,500 --> 00:02,000\n甲\n\n2\n01:02:03.250 --> 01:02:04.000\n乙\n";
    let cues = parse(text);
    assert_eq!(cues.len(), 2);
    assert_eq!(cues[0].start, Tick::from_millis(1500));
    assert_eq!(cues[0].end, Tick::from_millis(2000));
    assert_eq!(
        cues[1].start,
        Tick::from_millis((62 * 60 + 3) * 1000 + 250)
    );
    assert_eq!(cues[1].text, "乙");
}

#[test]
fn 去掉行内标签但保留正文() {
    let text =
        "1\n00:00:01,000 --> 00:00:02,000\n<i>斜体</i>与<b>粗体</b>\n{\\an8}顶部\n1 < 2 保留\n";
    let cues = parse(text);
    assert_eq!(cues.len(), 1);
    assert_eq!(cues[0].text, "斜体与粗体\n顶部\n1 < 2 保留");
}

#[test]
fn note_与_style_块都跳过() {
    let text = "WEBVTT\n\nNOTE 这是一段注释\n\nSTYLE\n::cue { color: red }\n\n00:00:01.000 --> 00:00:02.000\n正文\n";
    let cues = parse(text);
    assert_eq!(cues.len(), 1);
    assert_eq!(cues[0].text, "正文");
}

#[test]
fn 时间倒挂与空文本被丢掉() {
    let text = "1\n00:00:05,000 --> 00:00:04,000\n倒挂\n\n2\n00:00:06,000 --> 00:00:07,000\n\n3\n00:00:08,000 --> 00:00:09,000\n好的\n";
    let cues = parse(text);
    assert_eq!(cues.len(), 1);
    assert_eq!(cues[0].text, "好的");
}

#[test]
fn bom_与_crlf_都不影响解析() {
    let text = "\u{feff}1\r\n00:00:01,000 --> 00:00:02,000\r\n带 BOM\r\n\r\n";
    let cues = parse(text);
    assert_eq!(cues.len(), 1);
    assert_eq!(cues[0].text, "带 BOM");
}

#[test]
fn 格式按扩展名识别() {
    assert_eq!(
        Format::from_path(std::path::Path::new("a.SRT")),
        Some(Format::Srt)
    );
    assert_eq!(
        Format::from_path(std::path::Path::new("a.vtt")),
        Some(Format::Vtt)
    );
    assert_eq!(Format::from_path(std::path::Path::new("a.ass")), None);
    assert_eq!(Format::Srt.extension(), "srt");
}

#[test]
fn 排序后写出() {
    // 输入顺序乱掉，写出来必须按时间排：播放器按顺序找当前字幕。
    let cues = vec![s(5.0, 6.0, "后"), s(1.0, 2.0, "前")];
    let text = format_srt(&cues);
    let first = text.find("前").unwrap();
    let second = text.find("后").unwrap();
    assert!(first < second);
}

#[test]
fn 毫秒向下取整() {
    let cue = Cue {
        start: Tick::from_micros(1_999_999),
        end: Tick::from_secs(3.0),
        text: "x".to_string(),
    };
    let text = format_srt(&[cue]);
    assert!(
        text.contains("00:00:01,999"),
        "1.999999 秒应当写成 999 毫秒（宁早不晚）：{text}"
    );
}
