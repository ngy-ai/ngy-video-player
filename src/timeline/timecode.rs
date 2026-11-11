//! 时间码：`HH:MM:SS:FF` 的显示与解析。
//!
//! 为什么单独一个模块：时间码是「帧率的有理数表示」的显示形式——29.97 的**丢帧**写法
//! （`;` 分隔）与 30 的整帧写法只差一条公式，散在界面各处一定会写错。剪辑器里帧号
//! 是硬通货（修剪、跳转、对白点全靠它），所以格式化与解析都只从这里出。
//!
//! 两种写法：
//!
//! * 整帧（non-drop）：帧号除以名义帧率，`00:00:01:00` 就是第 30 帧（30fps）。
//! * 丢帧（drop-frame，NTSC 的 1000/1001 帧率用）：**不丢画面**，只是每分钟开头丢掉
//!   两个（59.94 是四个）帧号，让时间码跟上真实时间。23.976 / 24 没有这种写法。

use super::time::{Rational, Tick};

/// 这个帧率是否用丢帧写法显示（NTSC 的 29.97 / 59.94）。
pub fn prefers_drop_frame(fps: Rational) -> bool {
    // 只认 30000/1001 与 60000/1001：其他「差不多 29.97」的数值一律按整帧处理，
    // 免得给一个不该丢帧的序列生成跳号的显示。
    let value = fps.to_f64();
    [(30_000u32, 1001u32), (60_000, 1001)]
        .iter()
        .any(|(num, den)| {
            let target = *num as f64 / *den as f64;
            (value - target).abs() < 1e-6
        })
}

/// 名义帧率（丢帧公式里的「每秒几个号」）：29.97 → 30，59.94 → 60。
fn nominal(fps: Rational) -> i64 {
    fps.to_f64().round().max(1.0) as i64
}

/// 丢帧时每分钟开头要跳过的帧号个数：29.97 → 2，59.94 → 4。
fn drop_count(fps: Rational) -> i64 {
    (nominal(fps) / 15).max(0)
}

/// 把时间点写成时间码。`drop_frame` 只对 NTSC 帧率有效，其它帧率会被忽略。
pub fn format(tick: Tick, fps: Rational, drop_frame: bool) -> String {
    let nominal = nominal(fps);
    let mut frames = fps.round_frame_of(tick.max(Tick::ZERO));
    let drop_frame = drop_frame && prefers_drop_frame(fps);
    if drop_frame {
        // 把「真实帧号」换成「丢帧时间码里的帧号」：每分钟（除了第 10 分钟）
        // 少两个号，所以要往回补。注意十分钟的真实帧数是 round(fps × 600)
        // 而不是名义帧率的 600 倍——29.97 的十分钟是 17982 帧。
        let drop = drop_count(fps);
        let per_minute = nominal * 60 - drop;
        let per_ten_minutes = (fps.to_f64() * 600.0).round() as i64;
        let tens = frames / per_ten_minutes;
        let rest = frames % per_ten_minutes;
        frames += drop * 9 * tens;
        if rest > drop {
            frames += drop * ((rest - drop) / per_minute);
        }
    }
    let frame = frames % nominal;
    let total_seconds = frames / nominal;
    let second = total_seconds % 60;
    let minute = (total_seconds / 60) % 60;
    let hour = total_seconds / 3600;
    let separator = if drop_frame { ';' } else { ':' };
    format!("{hour:02}:{minute:02}:{second:02}{separator}{frame:02}")
}

/// 解析结果：时间点，以及用户用的是不是丢帧写法。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Parsed {
    pub tick: Tick,
    pub drop_frame: bool,
}

/// 解析时间码。
///
/// 接受 `HH:MM:SS:FF`，也接受省掉最左边几级的 `时:分:秒`、`分:秒`、`秒`——省略时
/// 最右边那段是**秒**，只有四段写法里最右边才是帧（剪辑里手敲 `1:30` 想的是 1 分 30 秒，
/// 不是第 1 秒第 30 帧）。`;` 分隔的丢帧写法只要带帧就得四段；丢帧只在 NTSC 帧率下成立。
/// 负号不支持——时间线没有负时间。
pub fn parse(text: &str, fps: Rational) -> Result<Parsed, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("时间码是空的".to_string());
    }
    let drop_frame = text.contains(';');
    let mut separators = 0;
    for ch in text.chars() {
        if ch == ':' || ch == ';' {
            separators += 1;
        } else if !ch.is_ascii_digit() {
            return Err(format!("时间码里有不能识别的字符：{ch}"));
        }
    }
    if separators > 3 {
        return Err("时间码最多四段：时:分:秒:帧".to_string());
    }
    if drop_frame && !prefers_drop_frame(fps) {
        return Err(format!(
            "{} 帧不支持丢帧写法（只有 29.97 / 59.94 用）",
            fps.to_f64()
        ));
    }

    let mut parts: Vec<i64> = Vec::new();
    for part in text.split([':', ';']) {
        if part.is_empty() {
            return Err("时间码里有空的一段".to_string());
        }
        let value: i64 = part
            .parse()
            .map_err(|_| format!("这一段不是数字：{part}"))?;
        parts.push(value);
    }
    // 段数决定每段是什么：四段才有时分秒帧，少一段就少最左边那一级。
    // 这样 `12:05` 是「12 分 5 秒」，而 `00:00:12:05` 才是 12 秒 5 帧。
    let (hour, minute, second, frame) = match parts.len() {
        4 => (parts[0], parts[1], parts[2], parts[3]),
        3 => (parts[0], parts[1], parts[2], 0),
        2 => (0, parts[0], parts[1], 0),
        _ => (0, 0, parts[0], 0),
    };
    let nominal = nominal(fps);
    if minute >= 60 || second >= 60 {
        return Err("分和秒都要小于 60".to_string());
    }
    if frame >= nominal {
        return Err(format!("帧号要在 0…{} 之间", nominal - 1));
    }

    let mut frames = ((hour * 60 + minute) * 60 + second) * nominal + frame;
    if drop_frame {
        // 丢帧写法：把跳过的号减掉才是真实帧号。
        let drop = drop_count(fps);
        let minutes = hour * 60 + minute;
        frames -= drop * (minutes - minutes / 10);
    }
    if frames < 0 {
        return Err("时间码落在序列之前".to_string());
    }
    Ok(Parsed {
        tick: fps.tick_at_frame(frames),
        drop_frame,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fps(num: u32, den: u32) -> Rational {
        Rational::new(num, den).unwrap()
    }

    fn ntsc() -> Rational {
        fps(30_000, 1001)
    }

    #[test]
    fn 整帧写法按名义帧率切分() {
        let thirty = fps(30, 1);
        assert_eq!(format(Tick::ZERO, thirty, false), "00:00:00:00");
        assert_eq!(format(Tick::from_secs(1.0), thirty, false), "00:00:01:00");
        // 第 29 帧还在第 0 秒里，第 30 帧进下一秒。
        assert_eq!(
            format(thirty.tick_at_frame(29), thirty, false),
            "00:00:00:29"
        );
        assert_eq!(
            format(thirty.tick_at_frame(30), thirty, false),
            "00:00:01:00"
        );
        assert_eq!(
            format(thirty.tick_at_frame(30 * 3661 + 7), thirty, false),
            "01:01:01:07"
        );
    }

    #[test]
    fn 二十三点九七六不是丢帧() {
        let film = fps(24_000, 1001);
        assert!(!prefers_drop_frame(film));
        // 10 秒 ≈ 第 240 帧，24 帧一格正好落在 00:00:10:00。
        assert_eq!(format(Tick::from_secs(10.0), film, true), "00:00:10:00");
        // 冒号写法的要求不受影响。
        assert!(parse("00:00:00;01", film).is_err());
    }

    #[test]
    fn 丢帧写法跳过每分钟的两个号() {
        let ntsc = ntsc();
        assert!(prefers_drop_frame(ntsc));
        // 真实第 1800 帧（一分钟）显示成 00:01:00;02——两个号被跳过。
        assert_eq!(format(ntsc.tick_at_frame(1800), ntsc, true), "00:01:00;02");
        // 第 10 分钟不跳号。
        assert_eq!(
            format(ntsc.tick_at_frame(17_982), ntsc, true),
            "00:10:00;00"
        );
        // 丢帧只是显示：同一时刻的整帧写法仍然连续。
        assert_eq!(format(ntsc.tick_at_frame(1800), ntsc, false), "00:01:00:00");
    }

    #[test]
    fn 丢帧时间码能原样解析回去() {
        let ntsc = ntsc();
        for frame in [0i64, 1799, 1800, 17_981, 17_982, 107_892] {
            let tick = ntsc.tick_at_frame(frame);
            let text = format(tick, ntsc, true);
            let parsed = parse(&text, ntsc).unwrap();
            assert_eq!(
                ntsc.round_frame_of(parsed.tick),
                frame,
                "{text} 应该回到第 {frame} 帧"
            );
            assert!(parsed.drop_frame);
        }
    }

    #[test]
    fn 解析接受省略与短写() {
        let thirty = fps(30, 1);
        // 两段是「分:秒」，三段是「时:分:秒」，四段才带帧。
        assert_eq!(parse("1:02", thirty).unwrap().tick, Tick::from_secs(62.0));
        assert_eq!(
            parse("00:00:02", thirty).unwrap().tick,
            Tick::from_secs(2.0)
        );
        assert_eq!(
            parse(" 00:00:00:15 ", thirty).unwrap().tick,
            thirty.tick_at_frame(15)
        );
    }

    #[test]
    fn 解析会拒绝越界与非法字符() {
        let thirty = fps(30, 1);
        assert!(parse("", thirty).is_err());
        assert!(parse("00:60:00:00", thirty).is_err(), "分不能到 60");
        assert!(
            parse("00:00:00:30", thirty).is_err(),
            "帧号不能等于名义帧率"
        );
        assert!(parse("00:00:0a:00", thirty).is_err());
        assert!(parse("1:2:3:4:5", thirty).is_err());
        assert!(parse("::", thirty).is_err());
    }
}
