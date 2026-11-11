//! 序列字幕 → ASS 脚本文本。
//!
//! ffmpeg 的 `subtitles` 滤镜（烧录）与 `-c:s ass / mov_text`（软字幕）都吃这份文本，
//! 排版交给 libass；预览侧另走界面叠层，不复用这里（界面没法跑 libass）。
//!
//! `[Script Info]` 的 `PlayResX/Y` 取序列分辨率：字号因此按序列尺寸定义，导 4K 还是
//! 1080p 观感一致，不会因为输出容器变了就换一套排版。

use crate::timeline::{Sequence, Tick};

use super::{SubtitleItem, SubtitleStyle, items};

/// 把序列里能落到 `[from, to)` 的字幕写成 ASS 文本。没有条目时返回 `None`：
/// 调用方据此不发字幕参数，也不写临时文件。
///
/// `range` 有值时（只导工作区）先裁到区间，再把时间整体减去入点：成片经过
/// `setpts=PTS-STARTPTS` 从 0 起算，字幕必须跟着一起挪，否则会晚整整一个入点。
pub fn ass_script(sequence: &Sequence, range: Option<(Tick, Tick)>) -> Option<String> {
    let (from, to) = range.unwrap_or((Tick::ZERO, sequence.duration()));
    if to <= from {
        return None;
    }

    let mut events: Vec<(&SubtitleItem, Tick, Tick)> = Vec::new();
    for item in items(sequence) {
        let start = item.start.max(from);
        let end = item.end.min(to);
        if start >= end {
            continue;
        }
        events.push((item, start - from, end - from));
    }
    if events.is_empty() {
        return None;
    }
    // 轨道靠上的先写；同一轨按起点排（工程加载时已排序，这里不依赖调用方）。
    events.sort_by_key(|(item, start, _)| (*start, item.id));

    // 同样的样式共用一条 Style 行：一个字幕轨几十条文本时 ASS 不至于膨胀一倍。
    let mut styles: Vec<&SubtitleStyle> = Vec::new();
    for (item, _, _) in &events {
        let style = &item.style;
        if !styles.contains(&style) {
            styles.push(style);
        }
    }

    let mut lines = vec![
        "[Script Info]".to_string(),
        "ScriptType: v4.00+".to_string(),
        format!("PlayResX: {}", sequence.width),
        format!("PlayResY: {}", sequence.height),
        "WrapStyle: 0".to_string(),
        "ScaledBorderAndShadow: yes".to_string(),
        "YCbCr Matrix: TV.709".to_string(),
        String::new(),
        "[V4+ Styles]".to_string(),
        "Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, \
BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, \
BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding"
            .to_string(),
    ];
    for (index, style) in styles.iter().enumerate() {
        lines.push(style_line(index, style));
    }
    lines.push(String::new());
    lines.push("[Events]".to_string());
    lines.push(
        "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text"
            .to_string(),
    );
    for (item, start, end) in events {
        let index = styles
            .iter()
            .position(|known| *known == &item.style)
            .unwrap_or(0);
        lines.push(format!(
            "Dialogue: 0,{},{},S{index},,0,0,0,,{}",
            ass_time(start),
            ass_time(end),
            escape_text(&item.text)
        ));
    }
    Some(lines.join("\n"))
}

fn style_line(index: usize, style: &SubtitleStyle) -> String {
    // ASS 的颜色是 AABBGGRR：不透明 00 + 反序的 RGB。描边固定压黑，阴影给一点透明黑。
    format!(
        "Style: S{index},{},{},{},&H000000FF,&H00000000,&H80000000,\
0,0,0,0,100,100,0,0,1,{},{},{},20,20,20,1",
        style.font,
        style.size,
        ass_color(style.color),
        style.outline,
        if style.outline > 0 { 1 } else { 0 },
        style.position.alignment(),
    )
}

/// ASS 的厘秒时间戳：`H:MM:SS.CC`。四舍五入到 10 毫秒（字幕能感知的最小粒度）。
fn ass_time(tick: Tick) -> String {
    let centis = (tick.micros().max(0) + 5_000) / 10_000;
    let hours = centis / 360_000;
    let minutes = (centis / 6_000) % 60;
    let seconds = (centis / 100) % 60;
    let fraction = centis % 100;
    format!("{hours}:{minutes:02}:{seconds:02}.{fraction:02}")
}

fn ass_color(rgb: u32) -> String {
    let r = (rgb >> 16) & 0xff;
    let g = (rgb >> 8) & 0xff;
    let b = rgb & 0xff;
    format!("&H00{b:02X}{g:02X}{r:02X}&")
}

/// ASS 文本字段里换一行是 `\N`；裸大括号会被当成样式覆盖标签，替换成圆括号。
fn escape_text(text: &str) -> String {
    text.replace('\r', "")
        .replace('\n', "\\N")
        .replace('{', "(")
        .replace('}', ")")
}
