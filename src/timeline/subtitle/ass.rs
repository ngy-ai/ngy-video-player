//! 序列字幕 → ASS 脚本文本。
//!
//! ffmpeg 的 `subtitles` 滤镜（烧录）与 `-c:s ass / mov_text`（软字幕）都吃这份文本，
//! 排版交给 libass；预览侧另走界面叠层，不复用这里（界面没法跑 libass）。
//!
//! `[Script Info]` 的 `PlayResX/Y` 取序列分辨率：字号档位按 1080p 定义（`STYLE_HEIGHT`），
//! 写进脚本时按序列高度折算，导 4K 还是 1080p 观感一致，不会因为输出容器变了就换一套排版。

use crate::timeline::{Sequence, Tick};

use super::{STYLE_HEIGHT, SubtitleItem, SubtitleStyle, items};

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
        lines.push(style_line(index, style, sequence.height));
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
        // 只导工作区把字幕裁断时，逐字节拍没法按比例切（被裁掉的那几个字没有对应
        // 时间段），退化成普通文字；整条字幕才写 `\kf` 标签。
        let clipped = item.start < from || item.end > to;
        let text = if clipped {
            escape_text(&item.text)
        } else {
            dialogue_text(item, ass_centis(end) - ass_centis(start))
        };
        lines.push(format!(
            "Dialogue: 0,{},{},S{index},,0,0,0,,{}",
            ass_time(start),
            ass_time(end),
            text
        ));
    }
    Some(lines.join("\n"))
}

fn style_line(index: usize, style: &SubtitleStyle, height: u32) -> String {
    // 字号档位按 1080p 定义，`PlayResY` 是序列高度：不折算的话 4K 序列上的字幕只有
    // 一半大（48 单位在 2160 高的画面上只占 2.2%）。导入侧按同一个比例折回去。
    let size = (style.size as f64 * height as f64 / STYLE_HEIGHT as f64)
        .round()
        .max(1.0);
    // ASS 的颜色是 AABBGGRR：不透明 00 + 反序的 RGB。描边固定压黑，阴影给一点透明黑。
    format!(
        "Style: S{index},{},{size},{},&H000000FF,&H00000000,&H80000000,\
0,0,0,0,100,100,0,0,1,{},{},{},20,20,20,1",
        style.font,
        ass_color(style.color),
        style.outline,
        if style.outline > 0 { 1 } else { 0 },
        style.position.alignment(),
    )
}

/// Tick → 厘秒（向下取整到四舍五入后的 10 毫秒粒度）。
fn ass_centis(tick: Tick) -> i64 {
    (tick.micros().max(0) + 5_000) / 10_000
}

/// ASS 的厘秒时间戳：`H:MM:SS.CC`。四舍五入到 10 毫秒（字幕能感知的最小粒度）。
fn ass_time(tick: Tick) -> String {
    let centis = ass_centis(tick);
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

/// 逐字字幕写成 `{\kf20}` 这样的卡拉 OK 标签：每个字一段，数字之和严格等于
/// Dialogue 的厘秒时长。没开逐字的条目走 `escape_text`，输出与旧脚本逐字节一致。
fn dialogue_text(item: &SubtitleItem, total_centis: i64) -> String {
    if !item.karaoke_enabled() {
        return escape_text(&item.text);
    }
    let parts = item.karaoke_centis(total_centis.max(0));
    let mut out = String::new();
    let mut index = 0usize;
    for ch in item.text.chars() {
        if ch == '\n' || ch == '\r' {
            out.push_str("\\N");
            continue;
        }
        let centis = parts.get(index).copied().unwrap_or(0);
        index += 1;
        out.push_str(&format!("{{\\kf{centis}}}"));
        match ch {
            '{' => out.push('('),
            '}' => out.push(')'),
            ch => out.push(ch),
        }
    }
    out
}
