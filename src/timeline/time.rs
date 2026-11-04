//! 时间基：`Tick`（i64 微秒）与 `Rational` 帧率。
//!
//! 全项目只有这里能把「第几帧」和「第几微秒」互相换算。素材帧率与序列帧率可以不同，
//! 换算只允许发生在明确的边界上，中间过程一律是整数。
//!
//! 几档非整数帧率（23.976 / 29.97 / 59.94）用有理数精确表示，不写成 `29.97`：
//! 后者只是 30000/1001 的近似值，按它算一小时会差出几十毫秒。
//!
//! 浮点只出现在两个边界上：从外部（ffmpeg 探测、用户输入）读入秒数、把时间显示成人
//! 能读的秒数。内部比较、累加、存盘全用整数。

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::ops::{Add, Sub};

/// 一秒等于多少微秒。所有换算的分母都从这里来。
pub const MICROS_PER_SECOND: i64 = 1_000_000;

/// 时间点或时长，单位微秒（1/1_000_000 秒）。
///
/// 序列上的位置、素材入出点、转场时长全部是它。负数在模型里不合法，但类型本身
/// 不阻止负数——校验放在编辑操作（`ops`）里，读工程时也走同一套校验。
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Tick(i64);

impl Tick {
    pub const ZERO: Tick = Tick(0);

    pub const fn from_micros(micros: i64) -> Self {
        Self(micros)
    }

    pub const fn micros(self) -> i64 {
        self.0
    }

    pub const fn from_millis(millis: i64) -> Self {
        Self(millis.saturating_mul(1_000))
    }

    /// 从外部读到的秒数换成 `Tick`。
    ///
    /// 只给「ffmpeg 报了个 12.345 秒」这种边界用。内部不要拿它做累加：
    /// 先转成整数微秒，之后每一步都是整数运算。
    pub fn from_secs(seconds: f64) -> Self {
        if !seconds.is_finite() {
            return Self::ZERO;
        }
        let micros = (seconds * MICROS_PER_SECOND as f64).round();
        if micros >= i64::MAX as f64 {
            Self(i64::MAX)
        } else if micros <= i64::MIN as f64 {
            Self(i64::MIN)
        } else {
            Self(micros as i64)
        }
    }

    /// 给界面显示、给 ffmpeg 传参用的秒数。不要拿它回来做比较。
    pub fn secs(self) -> f64 {
        self.0 as f64 / MICROS_PER_SECOND as f64
    }

    /// 第 `frames` 帧在序列上的时间点（按给定帧率）。
    pub fn from_frames(frames: i64, fps: Rational) -> Self {
        fps.tick_at_frame(frames)
    }

    /// 向下取整到第几帧：`[3.2 帧, 3.9 帧)` 都是第 3 帧。
    pub fn floor_frames(self, fps: Rational) -> i64 {
        fps.floor_frame_of(self)
    }

    /// 最近的一帧。帧→时间→帧往返就靠它还原。
    pub fn round_frames(self, fps: Rational) -> i64 {
        fps.round_frame_of(self)
    }

    /// 向上取整到第几帧：一个片段占了多少帧按它算才不会少一帧。
    pub fn ceil_frames(self, fps: Rational) -> i64 {
        fps.ceil_frame_of(self)
    }

    pub fn is_negative(self) -> bool {
        self.0 < 0
    }

    pub fn checked_add(self, other: Tick) -> Option<Tick> {
        self.0.checked_add(other.0).map(Tick)
    }

    pub fn checked_sub(self, other: Tick) -> Option<Tick> {
        self.0.checked_sub(other.0).map(Tick)
    }

    /// 溢出时贴边，不 panic。时间线上一旦溢出，贴边比崩掉更容易看出来。
    pub fn saturating_add(self, other: Tick) -> Tick {
        Tick(self.0.saturating_add(other.0))
    }

    pub fn saturating_sub(self, other: Tick) -> Tick {
        Tick(self.0.saturating_sub(other.0))
    }
}

impl Add for Tick {
    type Output = Tick;

    fn add(self, rhs: Tick) -> Tick {
        self.saturating_add(rhs)
    }
}

impl Sub for Tick {
    type Output = Tick;

    fn sub(self, rhs: Tick) -> Tick {
        self.saturating_sub(rhs)
    }
}

/// 帧率，精确的有理数（`30000/1001` 而不是 `29.97`）。
///
/// 构造时就约分，`den` 恒大于 0，所以两个 `Rational` 可以直接比相等：
/// `Rational::new(60_000, 2002)` 与 `Rational::new(30_000, 1001)` 是同一个值。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rational {
    num: u32,
    den: u32,
}

impl Rational {
    /// 23.976 fps（`24000/1001`）。
    pub const FPS_23_976: Rational = Rational::reduced(24_000, 1001);
    pub const FPS_24: Rational = Rational::reduced(24, 1);
    pub const FPS_25: Rational = Rational::reduced(25, 1);
    /// 29.97 fps（`30000/1001`）。
    pub const FPS_29_97: Rational = Rational::reduced(30_000, 1001);
    pub const FPS_30: Rational = Rational::reduced(30, 1);
    pub const FPS_50: Rational = Rational::reduced(50, 1);
    /// 59.94 fps（`60000/1001`）。
    pub const FPS_59_94: Rational = Rational::reduced(60_000, 1001);
    pub const FPS_60: Rational = Rational::reduced(60, 1);

    /// 已经约分的常量用这个：调用方保证 `gcd(num, den) == 1` 且两者非零。
    const fn reduced(num: u32, den: u32) -> Self {
        Self { num, den }
    }

    /// 构造并约分。分子或分母为 0 时返回 `None`——0 fps 没有意义。
    pub fn new(num: u32, den: u32) -> Option<Self> {
        if num == 0 || den == 0 {
            return None;
        }
        let divisor = gcd(num, den);
        Some(Self::reduced(num / divisor, den / divisor))
    }

    /// 解析 ffmpeg 的帧率写法：`30000/1001`、`25`，也接受 `29.97` 这种小数。
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if let Some((num, den)) = text.split_once('/') {
            let num: u32 = num.trim().parse().ok()?;
            let den: u32 = den.trim().parse().ok()?;
            return Self::new(num, den);
        }
        Self::from_f64(text.parse().ok()?)
    }

    /// 从探测到的十进制帧率还原有理数。
    ///
    /// 先吸附到常见档位（29.97 落在 `30000/1001`，误差 0.01% 以内），否则按千分之一
    /// 精度记录。吸附件很重要：ffmpeg 有的容器报 `29.97`，有的报 `30000/1001`，
    /// 不归一化的话同一份素材会被当成两种帧率。
    pub fn from_f64(fps: f64) -> Option<Self> {
        if !fps.is_finite() || fps <= 0.0 || fps > 1_000.0 {
            return None;
        }
        for rate in COMMON_FPS {
            if (fps - rate.to_f64()).abs() / rate.to_f64() < 1e-4 {
                return Some(*rate);
            }
        }
        Self::new((fps * 1_000.0).round() as u32, 1_000)
    }

    pub const fn num(self) -> u32 {
        self.num
    }

    pub const fn den(self) -> u32 {
        self.den
    }

    pub fn to_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// 第 `frame` 帧的时间点。四舍五入到微秒：一帧 33.4ms 的零头只能落在微秒上。
    pub fn tick_at_frame(self, frame: i64) -> Tick {
        // 分母是 num：tick = frame * 1e6 * den / num。用 i128 防中间结果溢出。
        let numerator = frame as i128 * MICROS_PER_SECOND as i128 * self.den as i128;
        let denominator = self.num as i128;
        // 加半个分母再向下取整 = 四舍五入；`div_euclid` 让负数也保持同一方向。
        let micros = (2 * numerator + denominator).div_euclid(2 * denominator);
        Tick(clamp_i64(micros))
    }

    pub fn floor_frame_of(self, tick: Tick) -> i64 {
        let numerator = tick.micros() as i128 * self.num as i128;
        let denominator = self.den as i128 * MICROS_PER_SECOND as i128;
        clamp_i64(numerator.div_euclid(denominator))
    }

    pub fn ceil_frame_of(self, tick: Tick) -> i64 {
        let numerator = tick.micros() as i128 * self.num as i128;
        let denominator = self.den as i128 * MICROS_PER_SECOND as i128;
        clamp_i64((-numerator).div_euclid(denominator).saturating_neg())
    }

    pub fn round_frame_of(self, tick: Tick) -> i64 {
        let numerator = tick.micros() as i128 * self.num as i128;
        let denominator = self.den as i128 * MICROS_PER_SECOND as i128;
        clamp_i64((2 * numerator + denominator).div_euclid(2 * denominator))
    }
}

impl fmt::Display for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.num, self.den)
    }
}

impl Serialize for Rational {
    /// 存成 `"30000/1001"`：工程文件是给人看的，能 diff、能手改。
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Rational {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| D::Error::custom(format!("不是合法的帧率：{text}")))
    }
}

/// 常见的帧率档位，给吸附与界面下拉用。
pub const COMMON_FPS: &[Rational] = &[
    Rational::FPS_23_976,
    Rational::FPS_24,
    Rational::FPS_25,
    Rational::FPS_29_97,
    Rational::FPS_30,
    Rational::FPS_50,
    Rational::FPS_59_94,
    Rational::FPS_60,
];

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let rest = a % b;
        a = b;
        b = rest;
    }
    a
}

fn clamp_i64(value: i128) -> i64 {
    value.clamp(i64::MIN as i128, i64::MAX as i128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一到十小时的每种帧率逐帧往返：错一帧这里就会挂。
    #[test]
    fn 帧到时间再回帧零漂移() {
        for fps in COMMON_FPS {
            for frames in [0, 1, 2, 23, 24, 25, 1000, 29_997, 1_000_003] {
                let tick = Tick::from_frames(frames, *fps);
                assert_eq!(
                    tick.round_frames(*fps),
                    frames,
                    "{fps} 下第 {frames} 帧往返丢了"
                );
            }

            // 十小时整的最后一帧。
            let frames = 10 * 3600 * fps.num() as i64 / fps.den() as i64;
            let tick = Tick::from_frames(frames, *fps);
            assert_eq!(tick.round_frames(*fps), frames, "{fps} 十小时处漂了");
        }
    }

    #[test]
    fn 一帧的时长按帧率换算() {
        assert_eq!(Tick::from_frames(1, Rational::FPS_25).micros(), 40_000);
        assert_eq!(Tick::from_frames(1, Rational::FPS_30).micros(), 33_333);
        // 30000/1001 的一帧是 33366.67µs，四舍五入到 33367。
        assert_eq!(Tick::from_frames(1, Rational::FPS_29_97).micros(), 33_367);
        // 29.97 下 30000 帧正好是 1001 秒：有理数算出来能落在整微秒上。
        assert_eq!(
            Tick::from_frames(30_000, Rational::FPS_29_97).micros(),
            1001 * MICROS_PER_SECOND
        );
        // 反过来，整秒不是整数帧：59.94 的一秒是 59.94 帧。
        assert_eq!(
            Tick::from_micros(MICROS_PER_SECOND).round_frames(Rational::FPS_59_94),
            60,
            "一秒按最近帧算是第 60 帧"
        );
    }

    #[test]
    fn 非整数帧率按有理数精确表示() {
        assert_eq!(Rational::parse("30000/1001"), Some(Rational::FPS_29_97));
        assert_eq!(Rational::FPS_29_97.num(), 30_000);
        assert_eq!(Rational::FPS_29_97.den(), 1001);
        assert_eq!(Rational::new(60_000, 2002), Rational::new(30_000, 1001));
        assert_eq!(Rational::from_f64(29.97), Some(Rational::FPS_29_97));
        assert_eq!(Rational::from_f64(23.976), Some(Rational::FPS_23_976));
        assert_eq!(Rational::from_f64(59.94), Some(Rational::FPS_59_94));
        assert_eq!(Rational::from_f64(25.0), Some(Rational::FPS_25));
        // 不是常见档位就按千分之一精度记，而不是吸到旁边的 30。
        assert_eq!(Rational::from_f64(25.5), Rational::new(51, 2));
        assert_eq!(Rational::parse("25"), Some(Rational::FPS_25));
    }

    #[test]
    fn 非法帧率被拒绝() {
        assert_eq!(Rational::new(0, 25), None);
        assert_eq!(Rational::new(25, 0), None);
        assert_eq!(Rational::parse("abc"), None);
        assert_eq!(Rational::parse("25/0"), None);
        assert_eq!(Rational::parse("-25"), None);
        assert_eq!(Rational::from_f64(0.0), None);
        assert_eq!(Rational::from_f64(f64::NAN), None);
        assert_eq!(Rational::from_f64(f64::INFINITY), None);
    }

    #[test]
    fn 取整三种方向() {
        let fps = Rational::FPS_25;
        // 3 帧 + 半个多一点点，落在第 3 帧和第 4 帧之间。
        let tick = Tick::from_frames(3, fps) + Tick::from_micros(30_000);
        assert_eq!(tick.floor_frames(fps), 3);
        assert_eq!(tick.round_frames(fps), 4);
        assert_eq!(tick.ceil_frames(fps), 4);
        assert_eq!(Tick::from_frames(4, fps).floor_frames(fps), 4);
        assert_eq!(Tick::ZERO.ceil_frames(fps), 0);
    }

    #[test]
    fn 负数时间保持整数取整语义() {
        let fps = Rational::FPS_25;
        // -0.75 帧。
        let tick = Tick::from_micros(-30_000);
        assert_eq!(tick.floor_frames(fps), -1, "向负无穷取整");
        assert_eq!(tick.ceil_frames(fps), 0);
        assert_eq!(tick.round_frames(fps), -1, "最近的一帧在负方向");
        // -0.25 帧，离第 0 帧更近。
        let quarter = Tick::from_micros(-10_000);
        assert_eq!(quarter.floor_frames(fps), -1);
        assert_eq!(quarter.round_frames(fps), 0);
    }

    #[test]
    fn 十小时边界() {
        let ten_hours = Tick::from_micros(10 * 3600 * MICROS_PER_SECOND);
        assert_eq!(ten_hours.micros(), 36_000_000_000);
        assert_eq!(ten_hours.round_frames(Rational::FPS_60), 10 * 3600 * 60);
        assert_eq!(
            Tick::from_frames(10 * 3600 * 60, Rational::FPS_60),
            ten_hours
        );
    }

    #[test]
    fn 秒数换算只在边界发生() {
        assert_eq!(Tick::from_secs(1.5).micros(), 1_500_000);
        assert_eq!(Tick::from_secs(-0.25).micros(), -250_000);
        assert_eq!(Tick::from_secs(f64::NAN), Tick::ZERO);
        assert_eq!(Tick::from_millis(1500), Tick::from_micros(1_500_000));
        assert_eq!(Tick::from_micros(1_500_000).secs(), 1.5);
    }

    #[test]
    fn 整数运算溢出时贴边() {
        let max = Tick::from_micros(i64::MAX);
        assert_eq!(max.saturating_add(Tick::from_micros(1)).micros(), i64::MAX);
        assert_eq!(max.checked_add(Tick::from_micros(1)), None);
        assert_eq!(Tick::ZERO.saturating_sub(Tick::from_micros(1)).micros(), -1);
    }

    #[test]
    fn 存盘是整数微秒与有理数分数() {
        let tick = Tick::from_frames(29_997, Rational::FPS_29_97);
        let text = serde_json::to_string(&tick).expect("序列化时间");
        assert_eq!(text, tick.micros().to_string(), "时间是裸整数微秒");
        assert_eq!(
            serde_json::from_str::<Tick>(&text).unwrap(),
            tick,
            "时间往返"
        );

        let rate = serde_json::to_string(&Rational::FPS_29_97).expect("序列化帧率");
        assert_eq!(rate, "\"30000/1001\"");
        assert_eq!(
            serde_json::from_str::<Rational>(&rate).unwrap(),
            Rational::FPS_29_97,
            "帧率往返"
        );
        assert!(serde_json::from_str::<Rational>("\"30/0\"").is_err());
    }
}
