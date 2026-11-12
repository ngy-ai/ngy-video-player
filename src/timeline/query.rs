//! 素材搜索与智能合集（M10-T08）：一套小查询语言，条件存在工程里、结果每次现算。
//!
//! 写法是一行空格分隔的条件，**空格之间是「并且」**：
//!
//! ```text
//! 航拍                      名称里含「航拍」
//! 名称:航拍 时长>60          两条都要满足（`名称:` 也可以省略）
//! -标签:废片                 否定：带这个标签的不要
//! 分辨率>=1920 编码:h264     字段可以用 >= / <= / > / < / =
//! 日期>2026-11-01            修改日期按本地日期比
//! ```
//!
//! 为什么不用正则 / 不做 OR：素材库就几十到几百个文件，用户的诉求是「把不要的挑掉」，
//! 否定 + 并且已经够用；上 OR 与括号就要做表达式树和错误提示，收益不成比例。
//!
//! 智能合集存的是**条件字符串**，不是结果 id：素材改了标签、重新探测过，下次打开合集
//! 自动跟着变，不用重新保存一遍。

use crate::timeline::{Asset, MediaInfo};
use serde::{Deserialize, Serialize};

/// 条件可以落在哪些字段上。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Name,
    Duration,
    Resolution,
    Fps,
    Codec,
    Date,
    Tag,
}

impl Field {
    /// 认得的字段名。中英文都收：用户不会为了搜素材去记英文键名。
    pub fn parse(text: &str) -> Option<Self> {
        let key = text.to_ascii_lowercase();
        Some(match key.as_str() {
            "名称" | "名字" | "name" => Field::Name,
            "时长" | "duration" | "长度" => Field::Duration,
            "分辨率" | "resolution" | "尺寸" => Field::Resolution,
            "帧率" | "fps" => Field::Fps,
            "编码" | "codec" => Field::Codec,
            "日期" | "date" | "修改时间" => Field::Date,
            "标签" | "tag" | "tags" => Field::Tag,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Field::Name => "名称",
            Field::Duration => "时长",
            Field::Resolution => "分辨率",
            Field::Fps => "帧率",
            Field::Codec => "编码",
            Field::Date => "日期",
            Field::Tag => "标签",
        }
    }
}

/// 一个条件。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Term {
    /// `-` 前缀：这条要「不满足」才算命中。
    pub negate: bool,
    pub field: Field,
    /// `必填` 表示值里没写操作符时用「包含」（名称 / 标签）或「等于」（数值）。
    pub op: Op,
    pub value: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// 包含（字符串）或相等（数值）。
    Contains,
    NotContains,
    Greater,
    Less,
    GreaterEqual,
    LessEqual,
}

impl Op {
    pub fn label(self) -> &'static str {
        match self {
            Op::Contains => "",
            Op::NotContains => "!=",
            Op::Greater => ">",
            Op::Less => "<",
            Op::GreaterEqual => ">=",
            Op::LessEqual => "<=",
        }
    }
}

/// 解析好的一串条件：全部满足才算命中。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub terms: Vec<Term>,
}

impl Query {
    /// 解析查询串。返回 `(查询, 认不出来的部分)`——认不出来的不静默丢掉，
    /// 界面要把它们摆出来，否则用户会以为条件生效了。
    pub fn parse(text: &str) -> (Self, Vec<String>) {
        let mut terms = Vec::new();
        let mut unknown = Vec::new();
        for word in text.split_whitespace() {
            // 否定前缀。`-` 后面紧跟字段名或裸词。
            let (negate, rest) = match word.strip_prefix('-') {
                Some(rest) if !rest.is_empty() => (true, rest),
                _ => (false, word),
            };
            // 字段前缀：`名称:xx` / `时长>60`。没有冒号也没有比较符时按裸词处理。
            let (field, op, value) = match rest.split_once(':') {
                Some((key, value)) => match Field::parse(key) {
                    Some(field) => (field, Op::Contains, value.to_string()),
                    None => (Field::Name, Op::Contains, rest.to_string()),
                },
                None => match split_compare(rest) {
                    Some((key, op, value)) => match Field::parse(key) {
                        Some(field) => (field, op, value.to_string()),
                        None => {
                            unknown.push(word.to_string());
                            continue;
                        }
                    },
                    // 带冒号 / 比较符却切不出值（`时长>`）：当写坏了报出来，
                    // 别当成文件名去搜——那样用户会以为条件生效了。
                    None if rest.contains([':', '>', '<', '=']) => {
                        unknown.push(word.to_string());
                        continue;
                    }
                    None => (Field::Name, Op::Contains, rest.to_string()),
                },
            };
            if value.is_empty() {
                unknown.push(word.to_string());
                continue;
            }
            // 数值字段上写 `时长:60` 就是「等于 60 秒」：`Op::Contains` 在数值比较里
            // 按相等处理（见 `compare`），不用在解析阶段换一种 op。
            terms.push(Term {
                negate,
                field,
                op,
                value,
            });
        }
        (Self { terms }, unknown)
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }
}

/// 从 `时长>60` 里切出 `(时长, >, 60)`。`>=` 要在 `>` 之前试。
fn split_compare(text: &str) -> Option<(&str, Op, &str)> {
    for (token, op) in [
        (">=", Op::GreaterEqual),
        ("<=", Op::LessEqual),
        ("!=", Op::NotContains),
        (">", Op::Greater),
        ("<", Op::Less),
        ("=", Op::Contains),
    ] {
        if let Some(index) = text.find(token) {
            let (key, value) = text.split_at(index);
            let value = &value[token.len()..];
            if key.is_empty() || value.is_empty() {
                return None;
            }
            return Some((key, op, value));
        }
    }
    None
}

impl Query {
    /// 这条素材满不满足。
    pub fn matches(&self, asset: &Asset) -> bool {
        self.terms.iter().all(|term| term.matches(asset))
    }
}

impl Term {
    fn matches(&self, asset: &Asset) -> bool {
        let hit = match self.field {
            Field::Name => {
                let name = asset.display_name().to_lowercase();
                let value = self.value.to_lowercase();
                match self.op {
                    Op::Contains | Op::GreaterEqual | Op::LessEqual => name.contains(&value),
                    Op::NotContains => !name.contains(&value),
                    Op::Greater | Op::Less => name.contains(&value),
                }
            }
            Field::Tag => {
                let value = self.value.to_lowercase();
                let hit = asset.tags.iter().any(|tag| tag.to_lowercase() == value);
                match self.op {
                    Op::NotContains => !hit,
                    _ => hit,
                }
            }
            Field::Codec => {
                let codec = asset
                    .media
                    .as_ref()
                    .map(|media| {
                        if media.has_video() {
                            media.video_codec.to_lowercase()
                        } else {
                            media.audio_codec.to_lowercase()
                        }
                    })
                    .unwrap_or_default();
                let value = self.value.to_lowercase();
                match self.op {
                    Op::NotContains => !codec.contains(&value),
                    _ => codec.contains(&value),
                }
            }
            Field::Duration => compare(
                number(&self.value),
                duration_seconds(asset.media.as_ref()),
                self.op,
            ),
            Field::Resolution => {
                // 分辨率按长边比：竖拍 1080×1920 与 1920×1080 视为同一档。
                let long = asset
                    .media
                    .as_ref()
                    .map(|media| {
                        let (width, height) = media.display_size();
                        width.max(height) as f64
                    })
                    .unwrap_or(0.0);
                compare(number(&self.value), long, self.op)
            }
            Field::Fps => {
                let fps = asset
                    .media
                    .as_ref()
                    .and_then(|media| media.fps)
                    .map(|fps| fps.to_f64())
                    .unwrap_or(0.0);
                compare(number(&self.value), fps, self.op)
            }
            Field::Date => {
                let day = asset_day(asset);
                let want = parse_day(&self.value);
                match (day, want) {
                    (Some(day), Some(want)) => compare_day(day, want, self.op),
                    _ => false,
                }
            }
        };
        if self.negate { !hit } else { hit }
    }
}

fn number(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok()
}

fn duration_seconds(media: Option<&MediaInfo>) -> f64 {
    media.map(|media| media.duration.secs()).unwrap_or(0.0)
}

/// 数值比较。任一侧读不出数字时一律不命中——宁可不显示，也不显示错的那条。
fn compare(want: Option<f64>, actual: f64, op: Op) -> bool {
    let Some(want) = want else {
        return false;
    };
    match op {
        Op::Contains | Op::GreaterEqual => actual >= want,
        Op::NotContains => (actual - want).abs() > f64::EPSILON,
        Op::Greater => actual > want,
        Op::Less => actual < want,
        Op::LessEqual => actual <= want,
    }
}

/// 素材的修改日期（本地时区的「年月日」），读不到就 `None`。
fn asset_day(asset: &Asset) -> Option<(i32, u32, u32)> {
    let stamp = asset.stamp?;
    Some(day_from_unix(stamp.modified))
}

/// Unix 秒 → `(年, 月, 日)`。不引 `chrono`：这里只要按天比，
/// 用「公历平均年长」算到天会有偏差，所以老实按 1970 起逐月推。
fn day_from_unix(seconds: i64) -> (i32, u32, u32) {
    let mut days = seconds.div_euclid(86_400);
    let mut year = 1970i32;
    loop {
        let leap = is_leap(year);
        let year_days = if leap { 366 } else { 365 };
        if days < year_days {
            break;
        }
        days -= year_days;
        year += 1;
    }
    let months = [
        31,
        if is_leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 0usize;
    while month < 12 && days >= months[month] {
        days -= months[month];
        month += 1;
    }
    (year, month as u32 + 1, days as u32 + 1)
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// `2026-11-01` / `2026/11/1` / `20261101` 三种都认。
fn parse_day(text: &str) -> Option<(i32, u32, u32)> {
    let cleaned = text.replace(['/', '.'], "-");
    let parts: Vec<&str> = cleaned.split('-').filter(|part| !part.is_empty()).collect();
    match parts.len() {
        3 => Some((
            parts[0].parse().ok()?,
            parts[1].parse().ok()?,
            parts[2].parse().ok()?,
        )),
        1 if parts[0].len() == 8 => Some((
            parts[0][0..4].parse().ok()?,
            parts[0][4..6].parse().ok()?,
            parts[0][6..8].parse().ok()?,
        )),
        _ => None,
    }
}

fn compare_day(day: (i32, u32, u32), want: (i32, u32, u32), op: Op) -> bool {
    let key = |(year, month, day): (i32, u32, u32)| (year, month, day);
    let (left, right) = (key(day), key(want));
    match op {
        Op::Contains | Op::GreaterEqual => left >= right,
        Op::NotContains => left != right,
        Op::Greater => left > right,
        Op::Less => left < right,
        Op::LessEqual => left <= right,
    }
}

/// 一个存条件（不存结果）的智能合集。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Collection {
    pub name: String,
    /// 查询串原样存下来：改条件就是改这行字，读盘时再解析。
    pub query: String,
}

impl Collection {
    pub fn new(name: impl Into<String>, query: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            query: query.into(),
        }
    }
}

#[cfg(test)]
mod tests;
