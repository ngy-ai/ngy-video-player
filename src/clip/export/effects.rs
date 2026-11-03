//! 时间重排与画面叠加：倒放、开头定格、文字水印。
//!
//! 这三项和「几何变换」不是一类事：裁切缩放改的是单帧的构图，这里改的是帧的
//! 先后顺序或者往画面上叠东西。单独成文件后，`export.rs` 只负责按正确顺序把
//! 它们拼进滤镜链。
//!
//! 倒放会把整段解码后的帧堆在内存里，只适合短片；这一点在面板上也要说清楚。

use std::path::{Path, PathBuf};

/// 水印的落点。
///
/// 九宫格里只留四个角加居中：面板宽度有限，再多的档位排不下，
/// 而「压在右下角」已经是九成场景。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WatermarkPosition {
    TopLeft,
    TopRight,
    #[default]
    BottomRight,
    BottomLeft,
    Center,
}

impl WatermarkPosition {
    pub const ALL: [WatermarkPosition; 5] = [
        WatermarkPosition::TopLeft,
        WatermarkPosition::TopRight,
        WatermarkPosition::BottomRight,
        WatermarkPosition::BottomLeft,
        WatermarkPosition::Center,
    ];

    pub fn label(self) -> &'static str {
        match self {
            WatermarkPosition::TopLeft => "左上",
            WatermarkPosition::TopRight => "右上",
            WatermarkPosition::BottomRight => "右下",
            WatermarkPosition::BottomLeft => "左下",
            WatermarkPosition::Center => "居中",
        }
    }

    /// drawtext 的 `x` / `y` 表达式。
    ///
    /// 留 24px 边距：贴边在播放器里会被裁掉半个字，4K 素材上也不算远。
    fn coords(self) -> (&'static str, &'static str) {
        match self {
            WatermarkPosition::TopLeft => ("24", "24"),
            WatermarkPosition::TopRight => ("w-tw-24", "24"),
            WatermarkPosition::BottomRight => ("w-tw-24", "h-th-24"),
            WatermarkPosition::BottomLeft => ("24", "h-th-24"),
            WatermarkPosition::Center => ("(w-tw)/2", "(h-th)/2"),
        }
    }
}

/// 文字水印。
#[derive(Clone, Debug, PartialEq)]
pub struct Watermark {
    /// 勾选状态。文字单独存，取消勾选再勾回来不用重打一遍。
    pub enabled: bool,
    /// 要画的字。为空时即使勾上也什么都不画。
    pub text: String,
    pub position: WatermarkPosition,
    /// 字号（输出画面的像素）。不跟分辨率走：4K 素材要手动调大。
    pub size: u32,
    /// 不透明度，0.05–1.0。
    pub opacity: f32,
}

impl Default for Watermark {
    fn default() -> Self {
        Self {
            enabled: false,
            text: String::new(),
            position: WatermarkPosition::default(),
            size: 28,
            opacity: 0.7,
        }
    }
}

impl Watermark {
    pub const MIN_SIZE: u32 = 12;
    pub const MAX_SIZE: u32 = 96;
    pub const SIZE_STEP: i32 = 4;
    /// 不透明度的步进（百分比）。
    pub const OPACITY_STEP: i32 = 10;

    /// 真的会画上去：勾了、有字。
    pub fn active(&self) -> bool {
        self.enabled && !self.text.trim().is_empty()
    }

    /// 画成一串 drawtext 参数；不满足条件时返回 `None`。
    ///
    /// `shadowx/shadowy` 给一层淡淡的黑影：白色文字压在明亮的画面上时，
    /// 没有描边就完全看不清，而画一个不透明底框又太难看。
    pub(super) fn filter(&self) -> Option<String> {
        if !self.active() {
            return None;
        }
        let (x, y) = self.position.coords();
        let size = self.size.clamp(Self::MIN_SIZE, Self::MAX_SIZE);
        let opacity = self.opacity.clamp(0.05, 1.0);
        Some(format!(
            "drawtext={}:text='{}':fontsize={size}:fontcolor=white@{opacity:.2}\
             :shadowcolor=black@0.6:shadowx=1:shadowy=1:x={x}:y={y}",
            font_option(),
            escape_drawtext(&self.text)
        ))
    }
}

/// 把文字写成 drawtext 的 `text` 值。
///
/// 值外面包了单引号，图层的逗号、分号不必再管；单引号里仍然要处理的是
/// drawtext 自己那一层转义：反斜杠、单引号、冒号、百分号。
fn escape_drawtext(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            // 换行在滤镜串里没法表达，换成空格：宁可挤成一行，也不要整条命令失败。
            '\n' | '\r' => escaped.push(' '),
            '\\' | '\'' | ':' | '%' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// 字体：优先用系统里现成的中文字体文件。
///
/// 不能只写 `font=` 交给 fontconfig：Windows 上很多 ffmpeg 构建没编 fontconfig，
/// 一写就是「Cannot find a valid font」。找一个确定存在的文件最稳。
fn font_option() -> String {
    match font_file() {
        Some(path) => format!("fontfile='{}'", filter_path(&path)),
        // 一个都没有时退回 fontconfig 的族名（Linux 发行版多数有）。
        None => "font=Sans".to_string(),
    }
}

fn font_file() -> Option<PathBuf> {
    let candidates: &[&str] = if cfg!(windows) {
        &[
            "C:/Windows/Fonts/msyh.ttc",
            "C:/Windows/Fonts/msyhbd.ttc",
            "C:/Windows/Fonts/simhei.ttf",
            "C:/Windows/Fonts/arial.ttf",
        ]
    } else if cfg!(target_os = "macos") {
        &[
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            "/Library/Fonts/Arial Unicode.ttf",
        ]
    } else {
        &[
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ]
    };
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

/// 把路径写成滤镜参数里的字面量。
///
/// 滤镜串用 `:` 分隔参数，Windows 的盘符冒号必须转义；反斜杠统一换成正斜杠，
/// 再整体包进单引号（由调用方负责），路径里的空格与括号就不用管了。
pub(super) fn filter_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .replace('\'', "\\'")
        .replace(':', "\\:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 没勾或没字时不画() {
        let watermark = Watermark::default();
        assert_eq!(watermark.filter(), None);

        let empty = Watermark {
            enabled: true,
            ..Watermark::default()
        };
        assert_eq!(empty.filter(), None);

        let blank = Watermark {
            enabled: true,
            text: "   ".into(),
            ..Watermark::default()
        };
        assert_eq!(blank.filter(), None);
    }

    #[test]
    fn 水印带上位置与透明度() {
        let watermark = Watermark {
            enabled: true,
            text: "示例".into(),
            position: WatermarkPosition::TopLeft,
            size: 36,
            opacity: 0.5,
        };
        let filter = watermark.filter().unwrap();
        assert!(filter.starts_with("drawtext="));
        assert!(filter.contains(":text='示例'"));
        assert!(filter.contains("fontsize=36"));
        assert!(filter.contains("fontcolor=white@0.50"));
        assert!(filter.contains("x=24:y=24"));
    }

    #[test]
    fn 水印文字转义滤镜语法字符() {
        let watermark = Watermark {
            enabled: true,
            text: "a:b 'c' 50% \\d\ne".to_string(),
            ..Watermark::default()
        };
        let filter = watermark.filter().unwrap();
        assert!(filter.contains(r"text='a\:b \'c\' 50\% \\d e'"), "{filter}");
    }

    #[test]
    fn 字号与透明度被夹到量程内() {
        let watermark = Watermark {
            enabled: true,
            text: "x".into(),
            size: 1000,
            opacity: 9.0,
            ..Watermark::default()
        };
        let filter = watermark.filter().unwrap();
        assert!(filter.contains("fontsize=96"));
        assert!(filter.contains("fontcolor=white@1.00"));
    }

    #[test]
    fn 滤镜里的路径转义盘符() {
        assert_eq!(
            filter_path(Path::new(r"C:\Windows\Fonts\a.ttc")),
            "C\\:/Windows/Fonts/a.ttc"
        );
    }
}
