//! 时间输入解析与区间校验：`90` / `1:30` / `01:02:03.500` / `01:02:03;15`。
//!
//! 两端都可以留空：开始留空表示从头，结束留空表示到最后。两端都留空时没有可裁的
//! 区间，界面会拒绝执行——这里不是「复制整份文件」的入口。
//!
//! 帧率只影响分号写法：帧号换算成秒要除以帧率，29.97 / 59.94 还要走丢帧规则。
//! 不知道帧率时分号写法一律判为非法，而不是猜一个帧率。

/// 两端时间输入框允许的最短区间。低于这个值的请求基本是误操作，也让进度条失去意义。
pub(crate) const MIN_DURATION: f64 = 0.05;

/// 输入时间文本的解析结果。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParsedTime {
    /// 留空：开始表示「从头」，结束表示「到最后」。
    Empty,
    Value(f64),
    Invalid,
}

/// 解析 `90`、`1:30`、`01:02:03.500` 三种写法。
///
/// 不知道帧率时用这个：分号写法的 SMPTE 时间码一律判为非法，而不是猜一个帧率。
pub fn parse_time(text: &str) -> ParsedTime {
    parse_time_with_fps(text, 0.0)
}

/// 带帧率的时间解析。
///
/// 帧率只影响分号写法：`01:02:03;15` 里的 15 是帧号，换算成秒要除以帧率。
/// 分号也是 SMPTE 的「丢帧（drop-frame）」标记，29.97 / 59.94 下按丢帧规则换算，
/// 这样从剪辑软件里抄来的时间码才对得上。
pub fn parse_time_with_fps(text: &str, fps: f64) -> ParsedTime {
    let text = text.trim();
    if text.is_empty() {
        return ParsedTime::Empty;
    }
    if let Some((clock, frames)) = text.split_once(';') {
        return parse_timecode(clock.trim(), frames.trim(), fps);
    }
    parse_clock(text)
}

/// SMPTE 时间码：`HH:MM:SS;FF`。
///
/// 必须写满时分秒三段：两段的写法（`1:30;5`）分不清是「1 分 30 秒」还是
/// 「1 小时 30 分」，与其猜，不如直接判错让用户改。
fn parse_timecode(clock: &str, frames: &str, fps: f64) -> ParsedTime {
    let Some((nominal, actual)) = nominal_fps(fps) else {
        return ParsedTime::Invalid;
    };
    let parts: Vec<&str> = clock.split(':').collect();
    if parts.len() != 3 {
        return ParsedTime::Invalid;
    }
    if parts
        .iter()
        .any(|part| part.is_empty() || !part.chars().all(|ch| ch.is_ascii_digit()))
    {
        return ParsedTime::Invalid;
    }
    let numbers: Vec<u32> = parts.iter().filter_map(|part| part.parse().ok()).collect();
    if numbers.len() != 3 {
        return ParsedTime::Invalid;
    }
    let (hours, minutes, seconds) = (numbers[0], numbers[1], numbers[2]);
    if minutes >= 60 || seconds >= 60 {
        return ParsedTime::Invalid;
    }
    if frames.is_empty() || !frames.chars().all(|ch| ch.is_ascii_digit()) {
        return ParsedTime::Invalid;
    }
    let Ok(frame) = frames.parse::<u32>() else {
        return ParsedTime::Invalid;
    };
    // 一个整秒里最多 nominal 帧，帧号等于帧率说明这一秒根本不存在。
    if frame >= nominal {
        return ParsedTime::Invalid;
    }

    let clock_seconds = (hours as f64 * 3600.0) + (minutes as f64 * 60.0) + seconds as f64;
    let total = if drop_frame(actual) {
        // 丢帧并不是真的丢画面：每分钟少数两个帧号（整十分钟除外），
        // 让时间码与 29.97 的实际时长对上。换算成「第几帧」再除以真实帧率。
        let total_minutes = hours as f64 * 60.0 + minutes as f64;
        let drop = if nominal >= 60 { 4.0 } else { 2.0 };
        let frame_number = clock_seconds * nominal as f64 + frame as f64
            - drop * (total_minutes - (total_minutes / 10.0).floor());
        frame_number / actual
    } else {
        clock_seconds + frame as f64 / actual
    };
    if total.is_finite() && total >= 0.0 {
        ParsedTime::Value(total)
    } else {
        ParsedTime::Invalid
    }
}

/// 时间码用的名义帧率（29.97 → 30）与真实帧率。帧率还没读出来时返回 `None`。
fn nominal_fps(fps: f64) -> Option<(u32, f64)> {
    if !fps.is_finite() || !(1.0..=240.0).contains(&fps) {
        return None;
    }
    Some((fps.round() as u32, fps))
}

/// 29.97 / 59.94 这两档在 SMPTE 里用丢帧时间码。
fn drop_frame(fps: f64) -> bool {
    (fps - 30_000.0 / 1001.0).abs() < 0.01 || (fps - 60_000.0 / 1001.0).abs() < 0.01
}

/// 不带帧号的时钟写法。
///
/// 冒号分段只有最后一段可以带小数；前面的段必须小于 60，避免 `1:75` 这种打错的
/// 写法被悄悄理解成 135 秒。
fn parse_clock(text: &str) -> ParsedTime {
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() > 3 {
        return ParsedTime::Invalid;
    }

    let mut total = 0.0;
    for (index, part) in parts.iter().enumerate() {
        let last = index + 1 == parts.len();
        if !digits_only(part, last) {
            return ParsedTime::Invalid;
        }
        let value: f64 = match part.parse() {
            Ok(value) => value,
            Err(_) => return ParsedTime::Invalid,
        };
        // 带冒号时最后一段也是「秒」，必须小于 60；只有单段写法才允许 `90` 这种
        // 「直接给秒数」的简写。
        if (!last || parts.len() > 1) && value >= 60.0 {
            return ParsedTime::Invalid;
        }
        total = total * 60.0 + value;
    }

    if total.is_finite() && total >= 0.0 {
        ParsedTime::Value(total)
    } else {
        ParsedTime::Invalid
    }
}

/// 只接受十进制数字，最后一段允许一个小数点（且不能以点结尾）。
fn digits_only(part: &str, allow_dot: bool) -> bool {
    if part.is_empty() || part.ends_with('.') {
        return false;
    }
    let mut dots = 0;
    let mut digits = 0;
    for ch in part.chars() {
        match ch {
            '0'..='9' => digits += 1,
            '.' if allow_dot => {
                dots += 1;
                if dots > 1 {
                    return false;
                }
            }
            _ => return false,
        }
    }
    digits > 0
}

/// 把秒数写成 `HH:MM:SS.mmm`，对话框里显示用。
pub fn format_time(seconds: f64) -> String {
    let seconds = seconds.max(0.0);
    let hours = (seconds / 3600.0).floor();
    let minutes = ((seconds - hours * 3600.0) / 60.0).floor();
    let rest = seconds - hours * 3600.0 - minutes * 60.0;
    format!("{hours:02.0}:{minutes:02.0}:{rest:06.3}")
}

/// 已换算成秒的裁切区间。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipRange {
    pub start: f64,
    pub end: f64,
}

impl ClipRange {
    pub fn duration(self) -> f64 {
        self.end - self.start
    }
}

/// 区间校验失败的原因。文案由这里给出，界面只负责显示。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeError {
    /// 两端都留空：没有可裁的区间。
    NothingToTrim,
    InvalidStart,
    InvalidEnd,
    /// 结束不晚于开始。
    StartNotBeforeEnd,
    /// 超出视频时长，或时长还没读出来。
    OutOfRange,
}

impl RangeError {
    pub fn message(self) -> &'static str {
        match self {
            RangeError::NothingToTrim => "开始和结束都留空，没有可裁切的区间",
            RangeError::InvalidStart => {
                "开始时间格式不对，可用 90、1:30、01:02:03.5、01:02:03;15（帧号）"
            }
            RangeError::InvalidEnd => {
                "结束时间格式不对，可用 90、1:30、01:02:03.5、01:02:03;15（帧号）"
            }
            RangeError::StartNotBeforeEnd => "结束时间必须晚于开始时间",
            RangeError::OutOfRange => "时间超出视频时长",
        }
    }
}

/// 把两段输入换算成裁切区间。
///
/// `Ok(None)` 表示两端都留空——调用方该禁用「开始裁切」，而不是复制整份文件。
pub fn range_from_inputs(
    start_text: &str,
    end_text: &str,
    duration: f64,
) -> Result<Option<ClipRange>, RangeError> {
    range_from_inputs_with_fps(start_text, end_text, duration, 0.0)
}

/// 同上，但知道片源帧率：分号写法的 SMPTE 时间码（`01:02:03;15`）会按帧号换算。
///
/// `fps <= 0` 表示帧率还没读出来，此时分号写法判为非法，而不是猜一个帧率。
pub fn range_from_inputs_with_fps(
    start_text: &str,
    end_text: &str,
    duration: f64,
    fps: f64,
) -> Result<Option<ClipRange>, RangeError> {
    let start = parse_time_with_fps(start_text, fps);
    let end = parse_time_with_fps(end_text, fps);

    if start == ParsedTime::Empty && end == ParsedTime::Empty {
        return Ok(None);
    }
    if start == ParsedTime::Invalid {
        return Err(RangeError::InvalidStart);
    }
    if end == ParsedTime::Invalid {
        return Err(RangeError::InvalidEnd);
    }
    if duration <= 0.0 {
        return Err(RangeError::OutOfRange);
    }

    let start = match start {
        ParsedTime::Value(value) => value,
        _ => 0.0,
    };
    // 结束留空就是「到最后」；比时长多出几十毫秒只是浮点误差，不当成越界。
    let end = match end {
        ParsedTime::Value(value) => value,
        _ => duration,
    };

    if end > duration + 0.05 {
        return Err(RangeError::OutOfRange);
    }
    let end = end.min(duration);
    if end - start < MIN_DURATION {
        return Err(RangeError::StartNotBeforeEnd);
    }
    Ok(Some(ClipRange { start, end }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 时间支持三种写法() {
        assert_eq!(parse_time("90"), ParsedTime::Value(90.0));
        assert_eq!(parse_time("1:30"), ParsedTime::Value(90.0));
        assert_eq!(parse_time("01:02:03.500"), ParsedTime::Value(3723.5));
        assert_eq!(parse_time(" 1:30 "), ParsedTime::Value(90.0));
        assert_eq!(parse_time("0.5"), ParsedTime::Value(0.5));
    }

    #[test]
    fn smpte_时间码按帧率换算() {
        // 25fps 的 `01:02:03;15`：第 15 帧，正好是 0.6 秒。
        assert_eq!(
            parse_time_with_fps("01:02:03;15", 25.0),
            ParsedTime::Value(3723.6)
        );
        // 24fps 的第 12 帧是 0.5 秒。
        assert_eq!(
            parse_time_with_fps("00:00:01;12", 24.0),
            ParsedTime::Value(1.5)
        );
        // 帧号必须小于帧率：25fps 里没有第 25 帧。
        assert_eq!(
            parse_time_with_fps("00:00:01;25", 25.0),
            ParsedTime::Invalid
        );
        // 不知道帧率时不猜。
        assert_eq!(parse_time("00:00:01;12"), ParsedTime::Invalid);
        // 时分秒必须写满三段。
        assert_eq!(parse_time_with_fps("1:02;5", 25.0), ParsedTime::Invalid);
        // 门限值：`00:01:00;02` 在 29.97 下是第 1800 帧，正好一分钟。
        let drop = parse_time_with_fps("00:01:00;02", 30_000.0 / 1001.0);
        match drop {
            ParsedTime::Value(seconds) => {
                assert!(
                    (seconds - 1800.0 * 1001.0 / 30_000.0).abs() < 0.001,
                    "{seconds}"
                );
            }
            other => panic!("应当解析成秒：{other:?}"),
        }
    }

    #[test]
    fn 留空是特殊值() {
        assert_eq!(parse_time(""), ParsedTime::Empty);
        assert_eq!(parse_time("   "), ParsedTime::Empty);
    }

    #[test]
    fn 认不出的时间不会被猜成数字() {
        for bad in [
            "abc", "1:", ":30", "1:2:3:4", "1:75", "-5", "1.2.3", "1:2.", ".", "1e3",
        ] {
            assert_eq!(parse_time(bad), ParsedTime::Invalid, "{bad}");
        }
    }

    #[test]
    fn 两端都留空表示不裁切() {
        assert_eq!(range_from_inputs("", "", 600.0), Ok(None));
        assert_eq!(range_from_inputs("  ", "", 600.0), Ok(None));
    }

    #[test]
    fn 只填一端时另一端取边界() {
        assert_eq!(
            range_from_inputs("", "90", 600.0),
            Ok(Some(ClipRange {
                start: 0.0,
                end: 90.0
            }))
        );
        assert_eq!(
            range_from_inputs("90", "", 600.0),
            Ok(Some(ClipRange {
                start: 90.0,
                end: 600.0
            }))
        );
        assert_eq!(
            range_from_inputs("1:30", "2:00", 600.0),
            Ok(Some(ClipRange {
                start: 90.0,
                end: 120.0
            }))
        );
    }

    #[test]
    fn 越界与倒序被拒绝() {
        assert_eq!(
            range_from_inputs("", "700", 600.0),
            Err(RangeError::OutOfRange)
        );
        assert_eq!(
            range_from_inputs("90", "80", 600.0),
            Err(RangeError::StartNotBeforeEnd)
        );
        assert_eq!(
            range_from_inputs("90", "90", 600.0),
            Err(RangeError::StartNotBeforeEnd)
        );
        assert_eq!(
            range_from_inputs("abc", "", 600.0),
            Err(RangeError::InvalidStart)
        );
        assert_eq!(
            range_from_inputs("", "abc", 600.0),
            Err(RangeError::InvalidEnd)
        );
        // 时长还没读出来时不能裁切。
        assert_eq!(
            range_from_inputs("0", "10", 0.0),
            Err(RangeError::OutOfRange)
        );
    }

    #[test]
    fn 时长格式化补零到毫秒() {
        assert_eq!(format_time(0.0), "00:00:00.000");
        assert_eq!(format_time(90.5), "00:01:30.500");
        assert_eq!(format_time(3723.25), "01:02:03.250");
    }
}
