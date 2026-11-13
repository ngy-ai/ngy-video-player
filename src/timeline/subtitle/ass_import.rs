//! ASS / SSA 脚本 → 带样式的字幕条目（导入侧）。
//!
//! 与 `ass.rs`（序列 → 脚本）互为往返：`[V4+ Styles]` 里的字体 / 字号 / 主色 / 描边 /
//! 对齐还原成 [`SubtitleStyle`]，`[Events]` 的 `Dialogue` 还原成时间与文字。SRT / VTT
//! 那条路只有文字（见 `text.rs`），样式要保住就只能走 ASS。
//!
//! 节内的列序按各自的 `Format:` 行认：Aegisub、Resolve、Premiere 写出来的列序不一样，
//! 按位置取值会静默错位。没有 `Format:` 行时才退回 ASS 的标准列序。
//!
//! 不指望还原一切：字距、旋转、粗斜体、`\pos` 这类覆盖标签在工程里没有对应物，读进来
//! 就丢掉。字号按 `PlayResY` 折回工程的 1080p 档位（见 `STYLE_HEIGHT`），否则 4K 脚本
//! 导进来会大一倍。

use std::collections::HashMap;
use std::path::Path;

use super::{FONTS, STYLE_HEIGHT, SubtitlePosition, SubtitleStyle};
use crate::timeline::Tick;

/// 导入出来的一条字幕：时间、文字，以及从样式表里认回来的排版。
#[derive(Clone, Debug, PartialEq)]
pub struct StyledCue {
    pub start: Tick,
    pub end: Tick,
    pub text: String,
    pub style: SubtitleStyle,
}

/// 读一整份脚本。文件读不出来才报错；块级问题（时间倒挂、空文本）一律跳过。
pub fn read(path: &Path) -> std::io::Result<Vec<StyledCue>> {
    let script = std::fs::read_to_string(path)?;
    Ok(parse(&script))
}

/// 解析一整份脚本。返回空列表表示「一个能用的 Dialogue 都没有」。
pub fn parse(script: &str) -> Vec<StyledCue> {
    let text = script.strip_prefix('\u{feff}').unwrap_or(script);
    let mut section = Section::Other;
    let mut play_res_y = STYLE_HEIGHT as f64;
    let mut style_columns = Columns::from_line(STYLE_COLUMNS);
    let mut event_columns = Columns::from_line(EVENT_COLUMNS);
    let mut raw_styles: Vec<RawStyle> = Vec::new();
    let mut raw_events: Vec<RawEvent> = Vec::new();

    for line in text.lines() {
        let line = line.trim_end_matches('\r').trim();
        // `;` 是 SSA / ASS 的注释行；`Comment:` 在下面按 key 跳过。
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            section = Section::of(name);
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if key.eq_ignore_ascii_case("Format") {
            match section {
                Section::Styles => style_columns = Columns::from_line(value),
                Section::Events => event_columns = Columns::from_line(value),
                _ => {}
            }
            continue;
        }
        match section {
            Section::Info => {
                if key.eq_ignore_ascii_case("PlayResY")
                    && let Ok(height) = value.parse::<f64>()
                    && height > 1.0
                {
                    play_res_y = height;
                }
            }
            Section::Styles => {
                if key.eq_ignore_ascii_case("Style")
                    && let Some(style) = RawStyle::parse(value, &style_columns)
                {
                    raw_styles.push(style);
                }
            }
            Section::Events => {
                if key.eq_ignore_ascii_case("Dialogue")
                    && let Some(event) = RawEvent::parse(value, &event_columns)
                {
                    raw_events.push(event);
                }
            }
            Section::Other => {}
        }
    }

    // 样式名大小写不敏感（ASS 规范如此），同名的按文件里先出现的算。
    let mut styles: HashMap<String, SubtitleStyle> = HashMap::new();
    for raw in &raw_styles {
        styles
            .entry(raw.name.to_ascii_lowercase())
            .or_insert_with(|| raw.to_style(play_res_y));
    }

    let mut cues: Vec<StyledCue> = raw_events
        .into_iter()
        .filter(|event| event.end > event.start)
        .map(|event| StyledCue {
            start: event.start,
            end: event.end,
            text: event.text,
            // 样式名对不上（手改过的脚本、`Default` 拼错）就落到工程默认样式上。
            style: styles
                .get(&event.style.to_ascii_lowercase())
                .cloned()
                .unwrap_or_default(),
        })
        .collect();
    cues.sort_by_key(|cue| cue.start);
    cues
}

/// 脚本里有意义的三个节。别的节（`[Fonts]`、`[Graphics]`）一律不看。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Info,
    Styles,
    Events,
    Other,
}

impl Section {
    fn of(name: &str) -> Self {
        let name = name.trim().to_ascii_lowercase();
        if name == "script info" {
            Self::Info
        } else if name.ends_with("styles") {
            // `[V4 Styles]` / `[V4+ Styles]` / `[V4++ Styles]` 差别只在字段集。
            Self::Styles
        } else if name.ends_with("events") {
            Self::Events
        } else {
            Self::Other
        }
    }
}

/// 一节里的列序。只按列名取值，所以多一列少一列都不会把值读串。
struct Columns {
    names: Vec<String>,
}

/// 没有 `Format:` 行时的标准列序，必须与 ASS 规范逐字段对齐：缺一项索引就全偏。
const STYLE_COLUMNS: &str = "Name,Fontname,Fontsize,PrimaryColour,SecondaryColour,OutlineColour,\
BackColour,Bold,Italic,Underline,StrikeOut,ScaleX,ScaleY,Spacing,Angle,BorderStyle,Outline,Shadow,\
Alignment,MarginL,MarginR,MarginV,Encoding";
const EVENT_COLUMNS: &str = "Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text";

impl Columns {
    fn from_line(value: &str) -> Self {
        let names: Vec<String> = value
            .split(',')
            .map(|name| name.trim().to_ascii_lowercase())
            .filter(|name| !name.is_empty())
            .collect();
        if names.is_empty() {
            Self::from_line(EVENT_COLUMNS)
        } else {
            Self { names }
        }
    }

    /// 按列数切开，最后一段留整串：`Text` 里带逗号是常态。
    fn fields<'a>(&self, value: &'a str) -> Vec<&'a str> {
        let count = self.names.len().max(1);
        let mut parts: Vec<&str> = value.splitn(count, ',').collect();
        while parts.len() < count {
            parts.push("");
        }
        parts
    }

    fn value<'a>(&self, fields: &[&'a str], name: &str) -> Option<&'a str> {
        let index = self.names.iter().position(|column| column == name)?;
        fields.get(index).map(|value| value.trim())
    }
}

/// 样式行里我们关心的几列。
struct RawStyle {
    name: String,
    font: String,
    size: f64,
    color: u32,
    outline: f64,
    alignment: u32,
}

impl RawStyle {
    fn parse(line: &str, columns: &Columns) -> Option<Self> {
        let fields = columns.fields(line);
        let name = columns.value(&fields, "name")?.to_string();
        if name.is_empty() {
            return None;
        }
        Some(Self {
            name,
            font: columns.value(&fields, "fontname").unwrap_or("").to_string(),
            size: columns
                .value(&fields, "fontsize")
                .and_then(|value| value.parse().ok())
                .unwrap_or(48.0),
            color: columns
                .value(&fields, "primarycolour")
                .and_then(parse_color)
                .unwrap_or(0x00ff_ffff),
            outline: columns
                .value(&fields, "outline")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0.0),
            alignment: columns
                .value(&fields, "alignment")
                .and_then(|value| value.parse().ok())
                .unwrap_or(2),
        })
    }

    /// 折回工程的样式：字号按 `PlayResY` 折算，其余靠 `normalize` 收口（字体不在面板
    /// 清单里会换成 Arial —— 工程只认那三种，libass 的任意字体没法在界面上编辑）。
    fn to_style(&self, play_res_y: f64) -> SubtitleStyle {
        let scale = if play_res_y > 1.0 {
            STYLE_HEIGHT as f64 / play_res_y
        } else {
            1.0
        };
        let mut style = SubtitleStyle {
            font: if self.font.trim().is_empty() {
                FONTS[0].to_string()
            } else {
                self.font.clone()
            },
            size: (self.size * scale).round().max(1.0) as u32,
            color: self.color,
            outline: self.outline.max(0.0).round() as u32,
            position: position_of(self.alignment),
        };
        style.normalize();
        style
    }
}

/// 一条 `Dialogue`：时间、文字与样式名（样式等全部行读完再解）。
struct RawEvent {
    style: String,
    start: Tick,
    end: Tick,
    text: String,
}

impl RawEvent {
    fn parse(line: &str, columns: &Columns) -> Option<Self> {
        let fields = columns.fields(line);
        let start = parse_time(columns.value(&fields, "start")?)?;
        let end = parse_time(columns.value(&fields, "end")?)?;
        let text = cue_text(columns.value(&fields, "text")?);
        if text.is_empty() {
            return None;
        }
        Some(Self {
            style: columns.value(&fields, "style").unwrap_or("").to_string(),
            start: start.max(Tick::ZERO),
            end,
            text,
        })
    }
}

/// `H:MM:SS.CC` → Tick。厘秒是 ASS 的最小粒度，补位规则同 SRT 的毫秒。
fn parse_time(stamp: &str) -> Option<Tick> {
    let (clock, fraction) = match stamp.split_once('.') {
        Some((clock, fraction)) => (clock, fraction),
        None => (stamp, ""),
    };
    let mut parts = clock.split(':');
    let mut numbers: Vec<&str> = parts.by_ref().collect();
    if numbers.len() < 2 || numbers.len() > 3 {
        return None;
    }
    let seconds: i64 = numbers.pop()?.trim().parse().ok()?;
    let minutes: i64 = numbers.pop()?.trim().parse().ok()?;
    let hours: i64 = numbers.pop().unwrap_or("0").trim().parse().ok()?;
    let centis: i64 = if fraction.is_empty() {
        0
    } else {
        let digits: String = fraction.chars().take(2).collect();
        format!("{digits:0<2}").parse().ok()?
    };
    if minutes >= 60 || seconds >= 60 {
        return None;
    }
    let total = ((hours * 60 + minutes) * 60 + seconds) * 1_000_000 + centis * 10_000;
    Some(Tick::from_micros(total.max(0)))
}

/// `&H00BBGGRR&` → `0xRRGGBB`。不透明度的那个字节（`&H80…`）丢掉：工程没有 alpha。
fn parse_color(value: &str) -> Option<u32> {
    let raw = value
        .trim()
        .trim_start_matches('&')
        .trim_start_matches(['H', 'h'])
        .trim_end_matches('&');
    let bgr = u32::from_str_radix(raw, 16).ok()? & 0x00ff_ffff;
    Some(((bgr & 0xff) << 16) | (bgr & 0x00ff00) | ((bgr >> 16) & 0xff))
}

/// ASS 的 `Alignment`（小键盘布局）→ 工程的三档位置。
fn position_of(alignment: u32) -> SubtitlePosition {
    match alignment {
        4..=6 => SubtitlePosition::Middle,
        7..=9 => SubtitlePosition::Top,
        _ => SubtitlePosition::Bottom,
    }
}

/// 事件文本 → 工程里的多行文字。
///
/// `{}` 里是覆盖标签（`\pos`、`\an8`、`\bord2`…），工程没有对应物，整段丢掉；
/// `\N` / `\n` 是硬换行，`\h` 是不换行空格。别的反斜杠序列（罕见）原样留着。
fn cue_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                for next in chars.by_ref() {
                    if next == '}' {
                        break;
                    }
                }
            }
            '\\' => match chars.next() {
                Some('N') | Some('n') => out.push('\n'),
                Some('h') => out.push(' '),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => {}
            },
            ch => out.push(ch),
        }
    }
    out.split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<&str>>()
        .join("\n")
}

#[cfg(test)]
mod tests;
