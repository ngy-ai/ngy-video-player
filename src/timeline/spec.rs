//! 序列规格：分辨率与帧率。
//!
//! 新建工程时选一次，之后在「序列规格」对话框里还能改。改规格只动像素与帧对齐，
//! 不碰任何片段的时间——`Tick` 是微秒，帧率只影响「第几帧」怎么换算成时间，
//! 所以 25fps 的工程改成 30fps 之后总时长一个微秒都不变。

use super::media::MediaInfo;
use super::time::Rational;
use std::fmt;

/// 宽高下限。libx264 的 `yuv420p` 要求色度成对取样，16 是它接受的最小边。
const MIN_SIDE: u32 = 16;
/// 宽高上限。libx264 的 level 6.2 最多 8192×4320，这里统一按 8192 卡。
const MAX_SIDE: u32 = 8192;

/// 一条序列的分辨率与帧率。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SequenceSpec {
    pub width: u32,
    pub height: u32,
    pub fps: Rational,
}

impl Default for SequenceSpec {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: Rational::FPS_30,
        }
    }
}

impl SequenceSpec {
    /// 对话框里的分辨率预设：`(宽, 高, 名字)`。
    pub const PRESETS: [(u32, u32, &'static str); 3] = [
        (1920, 1080, "1080p"),
        (3840, 2160, "4K"),
        (1080, 1920, "竖屏"),
    ];

    /// 对话框里的帧率预设。顺序按数值排，23.976 与 29.97 的名字在 `rate_label` 里。
    pub const RATES: [Rational; 8] = [
        Rational::FPS_23_976,
        Rational::FPS_24,
        Rational::FPS_25,
        Rational::FPS_29_97,
        Rational::FPS_30,
        Rational::FPS_50,
        Rational::FPS_59_94,
        Rational::FPS_60,
    ];

    pub fn new(width: u32, height: u32, fps: Rational) -> Self {
        Self { width, height, fps }
    }

    /// 按某个素材的规格建：尺寸取显示方向（手机竖拍的文件里是横的，要按旋转后的算），
    /// 帧率取素材自己的；素材没有画面时返回 `None`。
    pub fn from_media(media: &MediaInfo) -> Option<Self> {
        if !media.has_video() {
            return None;
        }
        let (width, height) = media.display_size();
        Some(Self {
            width,
            height,
            fps: media.fps.unwrap_or(Rational::FPS_30),
        })
    }

    /// 校验：宽高在编码器能吃下的范围内，且都是偶数。
    ///
    /// 奇数边长在 `yuv420p` 下没有对应的色度取样，libx264 会直接报错——
    /// 与其等导出跑了十分钟再失败，不如在这里拦下来。
    pub fn validate(&self) -> Result<(), SpecError> {
        for side in [self.width, self.height] {
            if side < MIN_SIDE {
                return Err(SpecError::TooSmall);
            }
            if side > MAX_SIDE {
                return Err(SpecError::TooLarge);
            }
            if side % 2 != 0 {
                return Err(SpecError::OddSide);
            }
        }
        Ok(())
    }

    /// 分辨率文案，例如 `1920×1080`。
    pub fn resolution_label(&self) -> String {
        format!("{}×{}", self.width, self.height)
    }

    /// 一行摘要，给对话框标题和标题条用。
    pub fn label(&self) -> String {
        format!("{} · {}", self.resolution_label(), rate_label(self.fps))
    }
}

/// 帧率的短写法：整数帧率不带小数，23.976 / 29.97 / 59.94 显示原始精度。
///
/// `Rational` 的 `Display` 是 `"30000/1001"`（存盘与 diff 用），界面上读起来太吵。
pub fn rate_label(fps: Rational) -> String {
    match fps {
        Rational::FPS_23_976 => "23.976".to_string(),
        Rational::FPS_29_97 => "29.97".to_string(),
        Rational::FPS_59_94 => "59.94".to_string(),
        other => {
            let value = other.to_f64();
            if (value - value.round()).abs() < 0.001 {
                format!("{}", value.round() as u32)
            } else {
                format!("{value:.2}")
            }
        }
    }
}

/// 规格为什么不合法。文案都带上了可执行的改法。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpecError {
    TooSmall,
    TooLarge,
    OddSide,
}

impl fmt::Display for SpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpecError::TooSmall => write!(f, "宽高不能小于 {MIN_SIDE} 像素"),
            SpecError::TooLarge => write!(f, "宽高不能大于 {MAX_SIDE} 像素"),
            SpecError::OddSide => write!(f, "宽高必须是偶数，改一个相近的偶数就好"),
        }
    }
}

impl std::error::Error for SpecError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 预设都在可用的范围内() {
        for (width, height, _) in SequenceSpec::PRESETS {
            let spec = SequenceSpec::new(width, height, Rational::FPS_30);
            assert!(spec.validate().is_ok(), "{width}×{height} 应当能用");
        }
        for fps in SequenceSpec::RATES {
            assert!(fps.to_f64() > 0.0);
        }
        assert_eq!(SequenceSpec::RATES.len(), 8, "验收要求的八挡帧率");
    }

    #[test]
    fn 奇数边长被拒绝() {
        let odd = SequenceSpec::new(1921, 1080, Rational::FPS_30);
        assert_eq!(odd.validate(), Err(SpecError::OddSide));
        assert!(odd.validate().unwrap_err().to_string().contains("偶数"));
        assert_eq!(
            SequenceSpec::new(8, 1080, Rational::FPS_30).validate(),
            Err(SpecError::TooSmall)
        );
        assert_eq!(
            SequenceSpec::new(19200, 1080, Rational::FPS_30).validate(),
            Err(SpecError::TooLarge)
        );
    }

    #[test]
    fn 按素材建规格时用旋转后的尺寸() {
        let portrait = MediaInfo {
            width: 1920,
            height: 1080,
            fps: Some(Rational::FPS_25),
            rotation: 90,
            ..MediaInfo::default()
        };
        let spec = SequenceSpec::from_media(&portrait).expect("有画面");
        assert_eq!((spec.width, spec.height), (1080, 1920));
        assert_eq!(spec.fps, Rational::FPS_25);

        // 没有画面就没有尺寸可抄。
        let audio = MediaInfo {
            channels: 2,
            ..MediaInfo::default()
        };
        assert!(SequenceSpec::from_media(&audio).is_none());
    }

    #[test]
    fn 帧率文案保留两位精度() {
        assert_eq!(rate_label(Rational::FPS_30), "30");
        assert_eq!(rate_label(Rational::FPS_23_976), "23.976");
        assert_eq!(rate_label(Rational::FPS_29_97), "29.97");
        assert_eq!(rate_label(Rational::FPS_59_94), "59.94");
        assert_eq!(
            SequenceSpec::new(1080, 1920, Rational::FPS_25).label(),
            "1080×1920 · 25"
        );
    }
}
