//! SRT / WebVTT 的读写（M10-T16 的「导入导出往返」）。
//!
//! 两种格式的结构几乎一样：一串「时间行 + 文本行」的块，块之间空行。差别只有
//! 时间码写法（`.` 与 `,`、小时可省略）与文件头（`WEBVTT`）。
//!
//! 往返的边界定在这里：
//!
//! * **只带文字与时间**。样式（字体 / 字号 / 颜色 / 位置）留在工程里，不写进 SRT——
//!   SRT 没有样式字段，VTT 的 `STYLE` 块各家播放器支持不一，写出去反而会让用户以为
//!   样式丢在了路上。ASS 才有完整的样式，那条路走 `subtitle/ass.rs`（用于导出烧录）。
//! * **读进来的行内标签会去掉**：`<i>`、`<b>`、`<font>`、`{\an8}` 这类排版指令在
//!   这边没有对应物，留着会原样显示在画面上。
//! * **读不出来的块整块跳过**，不报错：字幕文件里常见的 `NOTE` / `STYLE` / `REGION`
//!   块、序号对不上、时间倒挂，跳过比整份文件打不开有用。

use crate::timeline::Tick;
use std::path::Path;

/// 支持的字幕文件格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Srt,
    Vtt,
}

impl Format {
    /// 按扩展名认格式。认不出返回 `None`，由调用方给提示。
    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "srt" => Some(Self::Srt),
            "vtt" => Some(Self::Vtt),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Srt => "srt",
            Self::Vtt => "vtt",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Srt => "SRT",
            Self::Vtt => "WebVTT",
        }
    }
}

/// 一条字幕：时间与文字。样式不在这里（见模块注释）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cue {
    pub start: Tick,
    pub end: Tick,
    pub text: String,
}

/// 解析一整份字幕文件。
///
/// 返回空列表表示「一个能用的块都没有」——调用方据此提示，而不是往时间线上加
/// 一堆零长度条目。错误只用于「文件读不出来」。
pub fn parse(input: &str) -> Vec<Cue> {
    // BOM 与 CRLF 是 Windows 上最常见的两个坑：BOM 会让第一行多一个不可见字符，
    // CRLF 会让文本末尾多出 `\r`，两者都会把 `-->` 的解析带偏。
    let text = input.strip_prefix('\u{feff}').unwrap_or(input);
    let mut cues = Vec::new();
    let mut block: Vec<&str> = Vec::new();
    let mut skipped_header = false;
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        if line.trim().is_empty() {
            if !block.is_empty() {
                if let Some(cue) = parse_block(&block) {
                    cues.push(cue);
                }
                block.clear();
            }
            continue;
        }
        // `WEBVTT` 头只是文件标识；它后面的元数据行（`Kind: captions`）跟着块一起跳过。
        if !skipped_header && line.trim_start().starts_with("WEBVTT") {
            skipped_header = true;
            continue;
        }
        block.push(line);
    }
    if !block.is_empty()
        && let Some(cue) = parse_block(&block)
    {
        cues.push(cue);
    }
    cues.sort_by_key(|cue| cue.start);
    cues
}

/// 一个块：可选序号 / 标识行，时间行，若干文本行。
fn parse_block(lines: &[&str]) -> Option<Cue> {
    let arrow = lines
        .iter()
        .position(|line| line.contains("-->"))
        .filter(|index| *index <= 1)?;
    // 时间行之前的行只可能是序号（SRT）或 cue 标识（VTT），都不带信息。
    let (start, end) = parse_time_line(lines[arrow])?;
    let text: Vec<String> = lines[arrow + 1..]
        .iter()
        .map(|line| clean_text(line))
        .filter(|line| !line.is_empty())
        .collect();
    if text.is_empty() || end <= start {
        return None;
    }
    Some(Cue {
        start,
        end,
        text: text.join("\n"),
    })
}

/// `00:00:01,000 --> 00:00:03,500 align:start` → 两个时刻。箭头右边的设置忽略。
fn parse_time_line(line: &str) -> Option<(Tick, Tick)> {
    let (left, right) = line.split_once("-->")?;
    let start = parse_stamp(left.trim())?;
    // 右侧可能跟着 cue 设置（`align:start size:80%`），只取第一段。
    let end_raw = right.split_whitespace().next()?;
    let end = parse_stamp(end_raw)?;
    Some((start, end))
}

/// `[HH:]MM:SS[,.]mmm` → Tick。小时可以省略（VTT 常见）。
fn parse_stamp(stamp: &str) -> Option<Tick> {
    let stamp = stamp.trim();
    // 毫秒部分用 `.` 或 `,` 都收：两种格式的用户经常互相粘贴。
    let normalized = stamp.replace(',', ".");
    let mut parts = normalized.split(':');
    let mut numbers: Vec<&str> = parts.by_ref().collect();
    if numbers.len() < 2 || numbers.len() > 3 {
        return None;
    }
    let seconds = numbers.pop()?;
    let minutes = numbers.pop()?;
    let hours = numbers.pop().unwrap_or("0");
    let hours: i64 = hours.trim().parse().ok()?;
    let minutes: i64 = minutes.trim().parse().ok()?;
    let (whole, fraction) = match seconds.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (seconds, ""),
    };
    let seconds: i64 = whole.trim().parse().ok()?;
    // `,5` 是 500 毫秒：不足三位的按毫秒位左对齐补齐。
    let millis: i64 = if fraction.is_empty() {
        0
    } else {
        let digits: String = fraction.chars().take(3).collect();
        let padded = format!("{digits:0<3}");
        padded.parse().ok()?
    };
    if minutes >= 60 || seconds >= 60 {
        return None;
    }
    let total = ((hours * 60 + minutes) * 60 + seconds) * 1000 + millis;
    Some(Tick::from_millis(total.max(0)))
}

/// 去掉行内排版标签：`<i>` / `<b>` / `<font color=…>` / `{\an8}` 与几个实体。
///
/// 只认尖括号里的「看起来像标签」的东西，`1 < 2` 这种正文里的比较号留着。
fn clean_text(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '<' => {
                // 找得到 `>` 且中间没有空格以外的怪东西，就当成标签整段丢掉。
                let mut tag = String::new();
                let mut closed = false;
                for next in chars.by_ref() {
                    if next == '>' {
                        closed = true;
                        break;
                    }
                    tag.push(next);
                }
                if !closed || tag.trim().starts_with('=') {
                    out.push('<');
                    out.push_str(&tag);
                }
            }
            '{' => {
                // ASS 覆盖标签：`{\an8}`、`{\pos(1,2)}`。
                let mut body = String::new();
                let mut closed = false;
                for next in chars.by_ref() {
                    if next == '}' {
                        closed = true;
                        break;
                    }
                    body.push(next);
                }
                if !closed || !body.starts_with('\\') {
                    out.push('{');
                    out.push_str(&body);
                }
            }
            '&' => {
                let mut entity = String::new();
                let mut closed = false;
                for next in chars.by_ref() {
                    if next == ';' {
                        closed = true;
                        break;
                    }
                    if entity.len() > 8 {
                        break;
                    }
                    entity.push(next);
                }
                match (closed, entity.as_str()) {
                    (true, "amp") => out.push('&'),
                    (true, "lt") => out.push('<'),
                    (true, "gt") => out.push('>'),
                    (true, "nbsp") => out.push(' '),
                    _ => {
                        out.push('&');
                        out.push_str(&entity);
                    }
                }
            }
            ch => out.push(ch),
        }
    }
    out.trim().to_string()
}

/// 写成 SRT。行尾用 CRLF：播放器与剪辑软件对 `\n` 的容忍度不齐，CRLF 是唯一都认的。
pub fn format_srt(cues: &[Cue]) -> String {
    let mut out = String::new();
    let mut sorted: Vec<&Cue> = cues.iter().filter(|cue| cue.end > cue.start).collect();
    sorted.sort_by_key(|cue| cue.start);
    for (index, cue) in sorted.iter().enumerate() {
        out.push_str(&format!("{}\r\n", index + 1));
        out.push_str(&format!(
            "{} --> {}\r\n",
            stamp(cue.start, ','),
            stamp(cue.end, ',')
        ));
        for line in cue.text.lines() {
            out.push_str(line.trim_end());
            out.push_str("\r\n");
        }
        out.push_str("\r\n");
    }
    out
}

/// 写成 WebVTT。时间码用 `.`，头是 `WEBVTT`。
pub fn format_vtt(cues: &[Cue]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    let mut sorted: Vec<&Cue> = cues.iter().filter(|cue| cue.end > cue.start).collect();
    sorted.sort_by_key(|cue| cue.start);
    for cue in sorted {
        out.push_str(&format!(
            "{} --> {}\n",
            stamp(cue.start, '.'),
            stamp(cue.end, '.')
        ));
        for line in cue.text.lines() {
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

/// Tick → `HH:MM:SS{sep}mmm`。毫秒向下取整：宁可早 1ms 出现，也别晚 1ms。
fn stamp(at: Tick, separator: char) -> String {
    let millis = at.micros() / 1000;
    let hours = millis / 3_600_000;
    let minutes = (millis / 60_000) % 60;
    let seconds = (millis / 1000) % 60;
    let millis = millis % 1000;
    format!("{hours:02}:{minutes:02}:{seconds:02}{separator}{millis:03}")
}

/// 按格式写文件。返回写出的字节数（界面提示用）。
pub fn write(path: &Path, format: Format, cues: &[Cue]) -> std::io::Result<usize> {
    let text = match format {
        Format::Srt => format_srt(cues),
        Format::Vtt => format_vtt(cues),
    };
    std::fs::write(path, &text)?;
    Ok(text.len())
}

/// 读文件并解析。编码按 UTF-8 处理；这一步失败就是「文件不是 UTF-8 字幕」。
pub fn read(path: &Path) -> std::io::Result<Vec<Cue>> {
    let text = std::fs::read_to_string(path)?;
    Ok(parse(&text))
}

#[cfg(test)]
mod tests;
