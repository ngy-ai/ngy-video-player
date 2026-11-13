//! 逐字 / 卡拉 OK 的 ASS 往返：导出写 `\kf`，导入读回权重，工作区裁断时不硬写标签。

use super::*;
use crate::timeline::subtitle::parse_ass;

const HEADER: &str = "[Script Info]\nScriptType: v4.00+\nPlayResX: 1920\nPlayResY: 1080\n\n\
[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, Outline, Alignment\n\
Style: Default,Arial,48,&H00FFFFFF,2,2\n\n[Events]\n\
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n";

fn karaoke_item() -> SubtitleItem {
    let mut item = item(1.0, 2.0, "卡拉OK");
    item.set_karaoke(true);
    item.karaoke = vec![100, 300, 100, 100];
    item
}

fn dialogue(script: &str) -> &str {
    script
        .lines()
        .find(|line| line.starts_with("Dialogue: "))
        .unwrap()
}

#[test]
fn 开关逐字与播放头定位() {
    let mut item = karaoke_item();
    assert_eq!(
        item.karaoke_index(Tick::from_millis(500)),
        None,
        "还没到起点"
    );
    assert_eq!(item.karaoke_index(Tick::from_millis(1_100)), Some(0));
    assert_eq!(item.karaoke_index(Tick::from_millis(1_500)), Some(1));
    assert_eq!(
        item.karaoke_index(Tick::from_millis(2_000)),
        None,
        "出点不含"
    );
    item.set_karaoke(false);
    assert!(!item.karaoke_enabled());
    assert_eq!(item.karaoke_index(Tick::from_millis(1_100)), None);
    item.set_karaoke(true);
    assert_eq!(item.karaoke, vec![100, 100, 100, 100], "重新打开是匀速的");
}

#[test]
fn 每个字都带_kf_且厘秒之和等于整段时长() {
    let script = ass_script(&sequence_with(vec![karaoke_item()]), None).unwrap();
    let line = dialogue(&script);
    assert!(
        line.contains("{\\kf17}卡{\\kf50}拉{\\kf17}O{\\kf16}K"),
        "{line}"
    );
    let parts: Vec<i64> = line
        .split("\\kf")
        .skip(1)
        .map(|rest| {
            rest.chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap()
        })
        .collect();
    assert_eq!(parts.iter().sum::<i64>(), 100, "1 秒 = 100 厘秒：{parts:?}");
}

#[test]
fn 没开逐字的脚本不带任何标签() {
    let script = ass_script(&sequence_with(vec![item(1.0, 2.0, "卡拉OK")]), None).unwrap();
    assert!(dialogue(&script).ends_with(",,卡拉OK"));
    assert!(!script.contains('{'));
}

#[test]
fn 导回来的逐字权重能再写成同一份脚本() {
    let first = ass_script(&sequence_with(vec![karaoke_item()]), None).unwrap();
    let cues = parse_ass(&first);
    assert_eq!(cues.len(), 1);
    assert_eq!(cues[0].text, "卡拉OK");
    assert_eq!(cues[0].karaoke, vec![17, 50, 17, 16]);

    let mut restored = item(1.0, 2.0, &cues[0].text);
    restored.style = cues[0].style.clone();
    restored.karaoke = cues[0].karaoke.clone();
    let second = ass_script(&sequence_with(vec![restored]), None).unwrap();
    assert_eq!(dialogue(&first), dialogue(&second));
}

#[test]
fn 工作区把逐字字幕裁断时退化成普通文字() {
    let script = ass_script(
        &sequence_with(vec![karaoke_item()]),
        Some((Tick::from_millis(1_200), Tick::from_millis(1_800))),
    )
    .unwrap();
    assert!(dialogue(&script).ends_with(",,卡拉OK"), "{script}");
    assert!(!script.contains("\\kf"), "裁掉的字节拍没有对应的时间段");
}

#[test]
fn 第三方脚本的一个音节摊成每个字一份权重() {
    let script = format!(
        "{HEADER}Dialogue: 0,0:00:00.00,0:00:01.00,Default,,0,0,0,,{{\\kf30}}甲乙丙{{\\kf10}}丁"
    );
    let cues = parse_ass(&script);
    assert_eq!(cues[0].text, "甲乙丙丁");
    assert_eq!(cues[0].karaoke, vec![10, 10, 10, 10], "30 厘秒摊给三个字");
}

#[test]
fn 权重随工程_json_往返且老工程缺字段按没开逐字() {
    let item = karaoke_item();
    let json = serde_json::to_string(&item).unwrap();
    let back: SubtitleItem = serde_json::from_str(&json).unwrap();
    assert_eq!(back, item);

    let legacy: SubtitleItem =
        serde_json::from_str(r#"{"start":0,"end":3000000,"text":"老字幕"}"#).unwrap();
    assert!(!legacy.karaoke_enabled());
    assert_eq!(legacy.text, "老字幕");
}

#[test]
fn 覆盖标签与_k_混在一起也能读() {
    let script = format!(
        "{HEADER}Dialogue: 0,0:00:00.00,0:00:01.00,Default,,0,0,0,,{{\\an8\\k20}}甲{{\\k30}}乙"
    );
    let cues = parse_ass(&script);
    assert_eq!(cues[0].text, "甲乙");
    assert_eq!(cues[0].karaoke, vec![20, 30]);
}
