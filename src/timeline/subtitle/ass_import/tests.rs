//! ASS 导入的单测：钉住「第三方脚本能读进来」与「自己导出的脚本能原样读回」。

use super::*;
use crate::timeline::subtitle::ass_script;
use crate::timeline::{Id, Rational, Sequence, SubtitleItem, TrackKind};

const HEADER: &str = "[Script Info]\nScriptType: v4.00+\nPlayResX: 1920\nPlayResY: 1080\n\n\
[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, \
BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, \
Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n\
Style: Default,Microsoft YaHei,64,&H004AD5FF,&H000000FF,&H00000000,&H80000000,\
0,0,0,0,100,100,0,0,1,3,0,8,20,20,20,1\n\n\
[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n";

#[test]
fn 标准脚本能读回时间文字与样式() {
    let script = format!("{HEADER}Dialogue: 0,0:00:01.00,0:00:03.50,Default,,0,0,0,,第一句");
    let cues = parse(&script);
    assert_eq!(cues.len(), 1);
    assert_eq!(cues[0].start, Tick::from_secs(1.0));
    assert_eq!(cues[0].end, Tick::from_secs(3.5));
    assert_eq!(cues[0].text, "第一句");
    assert_eq!(cues[0].style.font, "Microsoft YaHei");
    assert_eq!(cues[0].style.size, 64);
    assert_eq!(cues[0].style.color, 0x00ff_d54a, "BGR 要翻回 RGB");
    assert_eq!(cues[0].style.outline, 3);
    assert_eq!(
        cues[0].style.position,
        SubtitlePosition::Top,
        "alignment 8 是上排"
    );
}

#[test]
fn 列序按_format_行认而不是按位置() {
    // Resolve 一类工具写出来的列序与标准不同：按位置读会把 Start 读成 Style。
    let script = "[V4+ Styles]\n".to_string()
        + "Format: Name, Fontname, Fontsize, PrimaryColour, Outline, Alignment\n\
Style: Wide,Noto Sans CJK SC,32,&H00FFFFFF,0,2\n\n\
[Events]\n\
Format: Layer, Style, Start, End, MarginL, MarginR, MarginV, Effect, Text\n\
Dialogue: 0,Wide,0:00:02.00,0:00:03.00,0,0,0,,带,逗号的文字";
    let cues = parse(&script);
    assert_eq!(cues.len(), 1);
    assert_eq!(cues[0].start, Tick::from_secs(2.0));
    assert_eq!(cues[0].end, Tick::from_secs(3.0));
    assert_eq!(cues[0].text, "带,逗号的文字", "Text 是最后一列，逗号留着");
    assert_eq!(cues[0].style.font, "Noto Sans CJK SC");
}

#[test]
fn 覆盖标签丢掉换行转义保留() {
    let script = format!(
        "{HEADER}Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,{{\\an8\\pos(320,240)}}第一行\
\\N第二行\\h收尾"
    );
    let cues = parse(&script);
    assert_eq!(cues[0].text, "第一行\n第二行 收尾");
}

#[test]
fn 缺样式与坏区间都跳过() {
    let script = format!(
        "{HEADER}Comment: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,注释不算\n\
Dialogue: 0,0:00:09.00,0:00:04.00,Default,,0,0,0,,时间倒挂\n\
Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,\n\
Dialogue: 0,0:00:05.00,0:00:06.00,不存在的样式,,0,0,0,,没有样式就用默认"
    );
    let cues = parse(&script);
    assert_eq!(cues.len(), 1, "{cues:?}");
    assert_eq!(cues[0].start, Tick::from_secs(5.0));
    assert_eq!(cues[0].style, SubtitleStyle::default());
}

#[test]
fn playres_不是_1080_时字号折回() {
    let script = "[Script Info]\nPlayResY: 2160\n\n[V4+ Styles]\n\
Format: Name, Fontname, Fontsize, PrimaryColour, Outline, Alignment\n\
Style: Default,Arial,96,&H00FFFFFF,2,2\n\n[Events]\n\
Format: Layer, Start, End, Style, Text\n\
Dialogue: 0,0:00:00.00,0:00:01.00,Default,4K 的一半";
    let cues = parse(script);
    assert_eq!(cues[0].style.size, 48, "96 按 2160 折回 1080 是 48");
}

fn sequence_with(items: Vec<SubtitleItem>) -> Sequence {
    let mut sequence = Sequence::new("字幕测试", Rational::FPS_30, 1920, 1080);
    let track = sequence.add_track(TrackKind::Subtitle);
    for item in items {
        sequence.track_mut(track).unwrap().subtitles.push(item);
    }
    sequence
}

fn item(id: u64, start: f64, end: f64, text: &str) -> SubtitleItem {
    SubtitleItem::new(
        Id::new(id),
        Tick::from_secs(start),
        Tick::from_secs(end),
        text,
    )
}

#[test]
fn 自己导出的脚本能原样读回() {
    let mut second = item(1001, 2.25, 3.5, "第二句\n换过行");
    second.style.size = 24;
    second.style.color = 0xff_d54a;
    second.style.outline = 1;
    second.style.position = SubtitlePosition::Middle;
    let sequence = sequence_with(vec![item(1000, 0.5, 2.0, "第一句"), second]);

    let script = ass_script(&sequence, None).unwrap();
    let cues = parse(&script);
    assert_eq!(cues.len(), 2);
    // 时间在脚本里只有厘秒精度：用例特意挑了 10ms 的整数倍。
    for (cue, original) in cues.iter().zip(["第一句", "第二句\n换过行"]) {
        assert_eq!(cue.text, original);
    }
    assert_eq!(cues[0].start, Tick::from_secs(0.5));
    assert_eq!(cues[1].end, Tick::from_secs(3.5));
    assert_eq!(cues[0].style, SubtitleStyle::default());
    assert_eq!(cues[1].style.size, 24);
    assert_eq!(cues[1].style.color, 0xff_d54a);
    assert_eq!(cues[1].style.outline, 1);
    assert_eq!(cues[1].style.position, SubtitlePosition::Middle);
}
