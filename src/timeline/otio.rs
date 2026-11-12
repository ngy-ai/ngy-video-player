//! OTIO 交换（M10-T17）：时间线与 OpenTimelineIO 的 `.otio` 互转。
//!
//! 目标不是「把工程原样存成另一种格式」——`.ngyproj` 才是工程文件。这里要的是
//! 与 Resolve / Premiere 这类剪辑软件换手：导出成它们认的 `Timeline`，导入它们
//! 给回来的时间线。所以只搬两边都有的东西：轨道顺序、片段顺序与入出点、空档、
//! 转场、标记；OTIO 表达不了的（字幕、效果栈、关键帧、调色、片尾定格、轨道音量）
//! 全部塞进 `metadata.ngy`——OTIO 规范把 `metadata` 定义为私有扩展区，不算改
//! schema，读不懂它的软件会原样忽略。
//!
//! 时间一律走 `RationalTime`：`value` 是帧数、`rate` 是帧率。工程内部仍是 [`Tick`]，
//! 换算是 [`tick_from`] / [`rational_time`] 两个函数的事，不在这里用 `f64` 秒累加。
//!
//! 文件分工：`export` 拼 JSON，`import` 读 JSON 并给出与工程无关的中间结构，
//! 由界面层决定素材怎么登记、序列怎么建。

mod export;
mod import;

pub use export::export_sequence;
pub use import::{
    ImportedClip, ImportedTimeline, ImportedTrack, ImportedTransition, import_timeline,
};

use crate::timeline::{Rational, Tick};
use serde_json::{Value, json};
use std::fmt;

/// OTIO 文件的扩展名（不带点）。
pub const OTIO_EXTENSION: &str = "otio";

/// `metadata.ngy` 的格式号。加字段不算新版本，语义变了才 +1。
pub const META_VERSION: u32 = 1;

/// 交换失败的原因。文案由这里给，界面只负责显示。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OtioError {
    /// 不是合法 JSON。
    Json(String),
    /// 顶层不是 `Timeline`。
    NotTimeline(String),
    /// 结构读得懂，但里面一条轨道都没有——导入进来也是空序列，干脆报错。
    NoTracks,
}

impl fmt::Display for OtioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OtioError::Json(message) => write!(f, "OTIO 文件不是合法 JSON：{message}"),
            OtioError::NotTimeline(found) => write!(
                f,
                "这个文件不是 OTIO 时间线（顶层是 {found}）；请选 `.otio` 的 Timeline 文件"
            ),
            OtioError::NoTracks => write!(f, "OTIO 时间线里没有任何轨道"),
        }
    }
}

impl std::error::Error for OtioError {}

/// 一个 `RationalTime` 值。`rate` 用序列帧率：OTIO 的 `value` 就是帧数。
pub(super) fn rational_time(tick: Tick, fps: Rational) -> Value {
    let rate = fps.to_f64();
    json!({
        "OTIO_SCHEMA": "RationalTime.1",
        "rate": rate,
        "value": tick.secs() * rate,
    })
}

/// 一个 `TimeRange` 值。
pub(super) fn time_range(start: Tick, duration: Tick, fps: Rational) -> Value {
    json!({
        "OTIO_SCHEMA": "TimeRange.1",
        "start_time": rational_time(start, fps),
        "duration": rational_time(duration, fps),
    })
}

/// 从 `RationalTime` 读回 [`Tick`]。坏值（缺字段、`rate` 为 0、NaN）退回 0。
///
/// 读文件时宁可把时间读成 0，也别让一处坏数据把整条时间线废掉：片段会被排到
/// 开头，用户看得见、也拖得动。
pub(super) fn tick_from(value: &Value) -> Tick {
    let frames = value
        .get("value")
        .and_then(Value::as_f64)
        .filter(|frames| frames.is_finite())
        .unwrap_or(0.0);
    let rate = value
        .get("rate")
        .and_then(Value::as_f64)
        .filter(|rate| rate.is_finite() && *rate > 0.0)
        .unwrap_or(0.0);
    if rate <= 0.0 {
        return Tick::ZERO;
    }
    Tick::from_secs(frames / rate)
}

/// `TimeRange` 的起点。缺 `start_time` 按 0（OTIO 允许省略）。
pub(super) fn range_start(value: &Value) -> Tick {
    value.get("start_time").map(tick_from).unwrap_or(Tick::ZERO)
}

/// `TimeRange` 的时长。
pub(super) fn range_duration(value: &Value) -> Tick {
    value.get("duration").map(tick_from).unwrap_or(Tick::ZERO)
}


/// 绝对路径 → `file://` URL。
///
/// 只做「非保留字符以外一律百分号转义」这一件事：Resolve 与 Premiere 都按这个
/// 认路径，中文名、空格、`#` 都不会被吃掉。盘符里的 `:` 保留原样（`file:///C:/…`）。
pub(super) fn file_url(path: &std::path::Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut url = String::from("file://");
    if !text.starts_with('/') {
        url.push('/');
    }
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                url.push(byte as char);
            }
            _ => url.push_str(&format!("%{byte:02X}")),
        }
    }
    url
}

/// `file://` URL → 路径。认不出来（`http://` 之类）时返回 `None`。
pub(super) fn path_from_url(url: &str) -> Option<std::path::PathBuf> {
    let rest = url
        .strip_prefix("file://")
        .or_else(|| url.strip_prefix("file:"))?;
    // `file:///C:/x` 去掉开头所有斜杠再按盘符判断；UNC（`file://server/share`）
    // 只能原样带上，剪 Windows 工程时用得到。
    let without_slashes = rest.trim_start_matches('/');
    // 先攒字节再按 UTF-8 解：URL 里的 `%E5%89%AA` 是 UTF-8 的三个字节，
    // 逐字节转 `char` 会得到三个拉丁字符（乱码）。
    let mut decoded = Vec::with_capacity(without_slashes.len());
    let bytes = without_slashes.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&without_slashes[index + 1..index + 3], 16)
        {
            decoded.push(byte);
            index += 3;
            continue;
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    let decoded = String::from_utf8_lossy(&decoded).into_owned();

    let had_leading_slash = rest.starts_with('/');
    let looks_like_windows = decoded.len() >= 2 && decoded.as_bytes()[1] == b':';
    if looks_like_windows {
        // C:/x：盘符前面不能带斜杠，`PathBuf` 才认成绝对路径。
        return Some(std::path::PathBuf::from(decoded));
    }
    if had_leading_slash {
        return Some(std::path::PathBuf::from(format!("/{decoded}")));
    }
    Some(std::path::PathBuf::from(decoded))
}

#[cfg(test)]
mod tests;
