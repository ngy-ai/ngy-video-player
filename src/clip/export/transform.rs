//! 导出时的画面变换：裁切、缩放、加黑边、旋转、翻转。
//!
//! 全部翻译成 ffmpeg 的视频滤镜，因此只在重编码时可用——流复制没有像素可以重排。
//! 顺序固定为「裁切 → 缩放 → 加黑边 → 旋转 → 翻转」：先裁掉不要的边，再按目标
//! 高度缩放（缩放后的画面再补边，边才不会被一起放大），旋转与翻转放最后，
//! 免得裁切坐标要跟着换坐标轴重算。
//!
//! 竖屏视频、手机拍摄的素材经常带着大段黑边或旋转标记，这一组开关就是给它们用的；
//! 播放端的旋转/镜像（`settings::Rotation`）只是显示层，不会写进导出文件。

/// 顺时针旋转。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rotate {
    #[default]
    None,
    Cw90,
    Cw180,
    Cw270,
}

impl Rotate {
    pub const ALL: [Rotate; 4] = [Rotate::None, Rotate::Cw90, Rotate::Cw180, Rotate::Cw270];

    pub fn label(self) -> &'static str {
        match self {
            Rotate::None => "不旋转",
            Rotate::Cw90 => "顺时针 90°",
            Rotate::Cw180 => "180°",
            Rotate::Cw270 => "顺时针 270°",
        }
    }
}

/// 目标分辨率。按高度定，宽度跟着宽高比走，避免把 21:9 的片子拉变形。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scale {
    #[default]
    Source,
    P1080,
    P720,
    P480,
    Half,
}

impl Scale {
    pub const ALL: [Scale; 5] = [
        Scale::Source,
        Scale::P1080,
        Scale::P720,
        Scale::P480,
        Scale::Half,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Scale::Source => "原尺寸",
            Scale::P1080 => "1080p",
            Scale::P720 => "720p",
            Scale::P480 => "480p",
            Scale::Half => "一半",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Scale::Source => "不改分辨率；只在裁切或补边之后仍按源尺寸输出",
            Scale::Half => "宽高各减半（四分之一像素数），例如 1080p 得到 540p",
            _ => "按高度缩放，宽度按宽高比自动取偶数",
        }
    }

    /// `-2` 让 ffmpeg 按宽高比算出偶数宽度：yuv420p 的编码器不接受奇数。
    fn filter(self) -> Option<&'static str> {
        match self {
            Scale::Source => None,
            Scale::P1080 => Some("scale=-2:1080"),
            Scale::P720 => Some("scale=-2:720"),
            Scale::P480 => Some("scale=-2:480"),
            Scale::Half => Some("scale=trunc(iw/4)*2:trunc(ih/4)*2"),
        }
    }
}

/// 补黑边到指定比例。
///
/// 与「缩放」相反：这里不改画面内容，只在四周加边，把任意比例的片子放进统一的
/// 画框里（投屏、混剪、上传平台时最常要 16:9）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pad {
    #[default]
    None,
    R16x9,
    R4x3,
    R1x1,
    R9x16,
}

impl Pad {
    pub const ALL: [Pad; 5] = [Pad::None, Pad::R16x9, Pad::R4x3, Pad::R1x1, Pad::R9x16];

    pub fn label(self) -> &'static str {
        match self {
            Pad::None => "不补边",
            Pad::R16x9 => "16:9",
            Pad::R4x3 => "4:3",
            Pad::R1x1 => "1:1",
            Pad::R9x16 => "9:16",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Pad::None => "保持原比例，不做补边",
            _ => "只加黑边、不裁画面：把整个画面装进这个比例的画框",
        }
    }

    fn ratio(self) -> Option<(u32, u32)> {
        match self {
            Pad::None => None,
            Pad::R16x9 => Some((16, 9)),
            Pad::R4x3 => Some((4, 3)),
            Pad::R1x1 => Some((1, 1)),
            Pad::R9x16 => Some((9, 16)),
        }
    }

    /// 宽高都取「原尺寸」与「按目标比例撑开后的尺寸」里较大的那个，再把画面居中。
    /// 逗号在滤镜串里是分隔符，表达式内的逗号必须转义成 `\,`。
    fn filter(self) -> Option<String> {
        let (rw, rh) = self.ratio()?;
        Some(format!(
            "pad=trunc(max(iw\\,ih*{rw}/{rh})/2)*2:trunc(max(ih\\,iw*{rh}/{rw})/2)*2:\
             (ow-iw)/2:(oh-ih)/2:black"
        ))
    }
}

/// 裁切的一条边。四边各自独立，界面按 2×2 摆。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Top,
    Right,
    Bottom,
}

impl Edge {
    pub const ALL: [Edge; 4] = [Edge::Left, Edge::Top, Edge::Right, Edge::Bottom];

    pub fn label(self) -> &'static str {
        match self {
            Edge::Left => "左",
            Edge::Top => "上",
            Edge::Right => "右",
            Edge::Bottom => "下",
        }
    }
}

/// 四边各裁掉多少（百分比，0–45）。
///
/// 用百分比而不是像素：同一个面板要服务 480p 与 4K 两种片子，写死像素在有缩放时
/// 反而算不准；黑边在所有分辨率下都占同样的比例。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Crop {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Crop {
    /// 单边上限。四边加起来还要给画面留出 10% 以上，否则 `crop` 会算出 0×0。
    pub const MAX: f32 = 45.0;
    /// 一次的步进量。
    pub const STEP: f32 = 1.0;

    pub fn is_empty(&self) -> bool {
        self.left <= 0.001 && self.top <= 0.001 && self.right <= 0.001 && self.bottom <= 0.001
    }

    /// 改一条边，保证「对边 + 本条」不超过 80%。
    pub fn nudge(&mut self, edge: Edge, delta: f32) {
        let (slot, opposite) = match edge {
            Edge::Left => (&mut self.left, self.right),
            Edge::Top => (&mut self.top, self.bottom),
            Edge::Right => (&mut self.right, self.left),
            Edge::Bottom => (&mut self.bottom, self.top),
        };
        let limit = (80.0 - opposite).min(Self::MAX);
        // 百分比显示到整数，步进必须取整，否则浮点累加会跑出 0.30000000000000004。
        let next = ((*slot + delta) * 10.0).round() / 10.0;
        *slot = next.clamp(0.0, limit);
    }

    fn filter(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let keep_w = 1.0 - (self.left + self.right) / 100.0;
        let keep_h = 1.0 - (self.top + self.bottom) / 100.0;
        let x = self.left / 100.0;
        let y = self.top / 100.0;
        // 宽高取偶数：编码器不接受奇数尺寸，`trunc(.../2)*2` 比四舍五入少一帧抖动。
        Some(format!(
            "crop=trunc(iw*{keep_w:.3}/2)*2:trunc(ih*{keep_h:.3}/2)*2:iw*{x:.3}:ih*{y:.3}"
        ))
    }
}

/// 一次导出的画面变换。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VideoTransform {
    pub crop: Crop,
    pub scale: Scale,
    pub pad: Pad,
    pub rotate: Rotate,
    pub flip_h: bool,
    pub flip_v: bool,
}

impl VideoTransform {
    /// 拼进 `-vf` 的那一段。没开任何一项时返回空，调用方据此省掉 `-vf`。
    pub fn filters(&self) -> Vec<String> {
        let mut filters = Vec::new();
        if let Some(crop) = self.crop.filter() {
            filters.push(crop);
        }
        if let Some(scale) = self.scale.filter() {
            filters.push(scale.to_string());
        }
        if let Some(pad) = self.pad.filter() {
            filters.push(pad);
        }
        match self.rotate {
            Rotate::None => {}
            Rotate::Cw90 => filters.push("transpose=1".to_string()),
            // 180° 用两次 90°：`transpose` 已经处理了旋转后的宽高交换与色彩空间标记，
            // 比 `hflip,vflip` 更接近播放器的观感。
            Rotate::Cw180 => {
                filters.push("transpose=1".to_string());
                filters.push("transpose=1".to_string());
            }
            Rotate::Cw270 => filters.push("transpose=2".to_string()),
        }
        if self.flip_h {
            filters.push("hflip".to_string());
        }
        if self.flip_v {
            filters.push("vflip".to_string());
        }
        filters
    }

    pub fn is_identity(&self) -> bool {
        self.crop.is_empty()
            && self.scale == Scale::Source
            && self.pad == Pad::None
            && self.rotate == Rotate::None
            && !self.flip_h
            && !self.flip_v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(transform: VideoTransform) -> String {
        transform.filters().join(",")
    }

    #[test]
    fn 默认不产生任何滤镜() {
        let transform = VideoTransform::default();
        assert!(transform.is_identity());
        assert!(transform.filters().is_empty());
    }

    #[test]
    fn 裁切按四边百分比算宽高与偏移() {
        let transform = VideoTransform {
            crop: Crop {
                left: 10.0,
                top: 5.0,
                right: 10.0,
                bottom: 5.0,
            },
            ..VideoTransform::default()
        };
        assert_eq!(
            chain(transform),
            "crop=trunc(iw*0.800/2)*2:trunc(ih*0.900/2)*2:iw*0.100:ih*0.050"
        );
    }

    #[test]
    fn 单边裁切也不会丢掉整块画面() {
        // 左边顶到上限时右边必须被压回去，否则宽高会变成 0。
        let mut crop = Crop {
            left: 40.0,
            right: 40.0,
            ..Crop::default()
        };
        crop.nudge(Edge::Left, 20.0);
        assert_eq!(crop.left, 40.0, "对边占 40% 时本条最多到 40%");
        assert!(crop.left + crop.right <= 80.0);
    }

    #[test]
    fn 缩放与补边的顺序是缩放在前() {
        let transform = VideoTransform {
            scale: Scale::P720,
            pad: Pad::R16x9,
            ..VideoTransform::default()
        };
        let chain = chain(transform);
        let scale = chain.find("scale=-2:720").expect("有缩放");
        let pad = chain.find("pad=").expect("有补边");
        assert!(scale < pad, "补边必须接在缩放后面：{chain}");
    }

    #[test]
    fn 旋转与翻转排在最后() {
        let transform = VideoTransform {
            rotate: Rotate::Cw90,
            flip_h: true,
            flip_v: true,
            ..VideoTransform::default()
        };
        assert_eq!(chain(transform), "transpose=1,hflip,vflip");
    }

    #[test]
    fn 一百八十度用两次九十度() {
        let transform = VideoTransform {
            rotate: Rotate::Cw180,
            ..VideoTransform::default()
        };
        assert_eq!(chain(transform), "transpose=1,transpose=1");
    }

    #[test]
    fn 补边表达式里的逗号要转义() {
        // 逗号在滤镜串里是分隔符，表达式内的 `\,` 少了就把整条链拆断了。
        let transform = VideoTransform {
            pad: Pad::R4x3,
            ..VideoTransform::default()
        };
        let chain = chain(transform);
        assert!(chain.contains("iw\\,ih*4/3"), "{chain}");
        assert!(chain.contains("ih\\,iw*3/4"), "{chain}");
    }
}
