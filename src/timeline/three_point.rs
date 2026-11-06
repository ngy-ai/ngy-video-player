//! 三点编辑的时间换算：源入点、源出点、时间线入点，定下要放多长一段。
//!
//! 三个点里任缺一个都能退化成两点或一点编辑：
//!
//! - 只有时间线入点 → 从素材头放到尾。
//! - 源入点 + 时间线入点 → 从源入点放到素材尾。
//! - 源入点 + 源出点 + 时间线入点 → 完整的三点编辑。
//!
//! 时间线出点（工作区终点）是**上限**不是第四点：它存在时按剩余空间截断，
//! 不够就提示，而不是把片段放到工作区外面去。

use super::time::Tick;

/// 三点编辑算出来的实际落位。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreePoint {
    pub source_in: Tick,
    /// 实际取用的源出点。因为时间线出点不够而被截断时小于调用方给的值。
    pub source_out: Tick,
    /// 时间线上的起点。
    pub at: Tick,
    /// 是否被时间线出点截断。界面据此提示「按剩余空间缩短了」。
    pub clipped: bool,
}

impl ThreePoint {
    pub fn duration(&self) -> Tick {
        self.source_out - self.source_in
    }
}

/// 三点编辑算不出来时的原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreePointError {
    /// 源出点不在源入点之后。
    EmptySource,
    /// 时间线落点不在时间线出点之前，没有空间可放。
    NoRoom,
    /// 落点或源入点为负。
    NegativeTime,
}

impl std::fmt::Display for ThreePointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThreePointError::EmptySource => {
                write!(
                    f,
                    "源入点必须在源出点前面；先在预览区拖到要用的位置再按设点"
                )
            }
            ThreePointError::NoRoom => {
                write!(f, "播放头已经越过时间线出点，先把它挪到出点前面")
            }
            ThreePointError::NegativeTime => write!(f, "时间不能为负"),
        }
    }
}

impl std::error::Error for ThreePointError {}

/// 算出实际要放的源区间。
///
/// `source_in` / `source_out` 是素材自己的时间轴，`at` 是序列上的落点，
/// `timeline_out` 是时间线出点（工作区终点），`None` 表示不限制。
pub fn three_point(
    source_in: Tick,
    source_out: Tick,
    at: Tick,
    timeline_out: Option<Tick>,
) -> Result<ThreePoint, ThreePointError> {
    if source_in.is_negative() || at.is_negative() {
        return Err(ThreePointError::NegativeTime);
    }
    if source_out <= source_in {
        return Err(ThreePointError::EmptySource);
    }

    let mut end = source_out;
    let mut clipped = false;
    if let Some(out) = timeline_out {
        if out <= at {
            return Err(ThreePointError::NoRoom);
        }
        let room = out - at;
        if room < source_out - source_in {
            end = source_in + room;
            clipped = true;
        }
    }

    Ok(ThreePoint {
        source_in,
        source_out: end,
        at,
        clipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: f64) -> Tick {
        Tick::from_secs(n)
    }

    #[test]
    fn 没有时间线出点时按源区间原样放() {
        let point = three_point(s(1.0), s(4.0), s(10.0), None).expect("算得出来");
        assert_eq!(point.source_in, s(1.0));
        assert_eq!(point.source_out, s(4.0));
        assert_eq!(point.at, s(10.0));
        assert_eq!(point.duration(), s(3.0));
        assert!(!point.clipped);
    }

    #[test]
    fn 时间线剩余空间不够时按剩余截断() {
        // 源区间 3 秒，工作区从 10 秒起只剩 1.5 秒。
        let point = three_point(s(1.0), s(4.0), s(10.0), Some(s(11.5))).expect("算得出来");
        assert_eq!(point.source_out, s(2.5), "从源入点往后取 1.5 秒");
        assert_eq!(point.duration(), s(1.5));
        assert!(point.clipped);
    }

    #[test]
    fn 空间够时不截断() {
        let point = three_point(s(0.0), s(2.0), s(0.0), Some(s(5.0))).expect("算得出来");
        assert_eq!(point.source_out, s(2.0));
        assert!(!point.clipped);
    }

    #[test]
    fn 落点在出点之后没有空间() {
        assert_eq!(
            three_point(s(0.0), s(2.0), s(5.0), Some(s(5.0))),
            Err(ThreePointError::NoRoom)
        );
        assert_eq!(
            three_point(s(0.0), s(2.0), s(6.0), Some(s(5.0))),
            Err(ThreePointError::NoRoom)
        );
    }

    #[test]
    fn 源区间为空或反向被拒绝() {
        assert_eq!(
            three_point(s(2.0), s(2.0), s(0.0), None),
            Err(ThreePointError::EmptySource)
        );
        assert_eq!(
            three_point(s(3.0), s(1.0), s(0.0), None),
            Err(ThreePointError::EmptySource)
        );
        assert!(ThreePointError::EmptySource.to_string().contains("源入点"));
    }
}
