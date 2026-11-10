//! 片段调色：亮度 / 对比度 / 饱和度 / 色温与 3D LUT。
//!
//! 参数存整数百分比（-100..=100），不存浮点：序列靠 `PartialEq` 判断「这一步有没有
//! 改动」，浮点在往返读写里会给出肉眼看不见的差异，撤销栈就会多出空操作。
//!
//! 只做一级校色，没有色轮、限定器与节点（见 `TODO.md` M9-T11 的「不做」清单）：
//! - 亮度 / 对比度 / 饱和度合成一条 `eq`，三个值一起给，少一次滤镜链拼接；
//! - 色温用 `colorbalance` 的红蓝偏移近似——`colortemperature` 要 ffmpeg 4.4+，
//!   而 `colorbalance` 从 2.x 就有，老构建上也能跑；
//! - LUT 用 `lut3d`，路径按工程目录解析（相对路径优先，跨盘符才退回绝对路径）。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 参数范围：-100..=100，0 表示不变。
pub const COLOR_RANGE: i32 = 100;
/// 面板上按一下加减多少。
pub const COLOR_STEP: i32 = 10;
/// 色温 ±100 对应的红蓝偏移量。给满会让画面明显发橙 / 发蓝，0.3 是能看出来但不脏的档。
const TEMPERATURE_SHIFT: f64 = 0.3;

/// 一个片段的调色参数。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorGrade {
    /// -100（压暗）..=100（提亮），映射到 `eq` 的 `brightness`。
    pub brightness: i32,
    /// -100（平）..=100（硬），映射到 `eq` 的 `contrast = 1 + value/100`。
    pub contrast: i32,
    /// -100（灰）..=100（艳），映射到 `eq` 的 `saturation = 1 + value/100`。
    pub saturation: i32,
    /// -100（冷）..=100（暖），映射到 `colorbalance` 的 `rs` / `bs`。
    pub temperature: i32,
    /// 3D LUT 文件（`.cube` / `.3dl` 等）。存的是可移植路径：相对工程目录优先。
    pub lut: Option<PathBuf>,
}

impl ColorGrade {
    /// 全默认：不插任何滤镜。
    pub fn is_neutral(&self) -> bool {
        self.brightness == 0
            && self.contrast == 0
            && self.saturation == 0
            && self.temperature == 0
            && self.lut.is_none()
    }

    /// 面板 / 提示里的一行摘要。全默认时说「原始」而不是列四个 0。
    pub fn summary(&self) -> String {
        if self.is_neutral() {
            return "原始色彩".to_string();
        }
        let mut parts = Vec::new();
        for (label, value) in [
            ("亮度", self.brightness),
            ("对比度", self.contrast),
            ("饱和度", self.saturation),
            ("色温", self.temperature),
        ] {
            if value != 0 {
                parts.push(format!("{label} {value:+}"));
            }
        }
        if let Some(name) = self.lut.as_ref().and_then(|path| path.file_name()) {
            parts.push(format!("LUT {}", name.to_string_lossy()));
        }
        parts.join(" · ")
    }

    /// 拼成滤镜链的一段（不带首尾逗号）。`base_dir` 是工程目录，用来解析 LUT 的相对路径。
    pub fn filter_chain(&self, base_dir: &Path) -> String {
        if self.is_neutral() {
            return String::new();
        }
        let mut parts: Vec<String> = Vec::new();
        let brightness = clamp(self.brightness);
        let contrast = clamp(self.contrast);
        let saturation = clamp(self.saturation);
        if brightness != 0 || contrast != 0 || saturation != 0 {
            parts.push(format!(
                "eq=brightness={:.3}:contrast={:.3}:saturation={:.3}",
                brightness as f64 / 100.0,
                1.0 + contrast as f64 / 100.0,
                1.0 + saturation as f64 / 100.0,
            ));
        }
        let temperature = clamp(self.temperature);
        if temperature != 0 {
            let shift = temperature as f64 / 100.0 * TEMPERATURE_SHIFT;
            parts.push(format!("colorbalance=rs={shift:.3}:bs={:.3}", -shift));
        }
        if let Some(lut) = &self.lut {
            // 位置参数不给 `filename=`：`lut3d` 的选项名是 `file`，而位置参数在
            // 所有版本上都认，更不容易因 ffmpeg 版本差异写错。
            parts.push(format!("lut3d='{}'", filter_path(&resolve(base_dir, lut))));
        }
        parts.join(",")
    }
}

/// 夹到 -100..=100：手改坏的工程不该让整条导出失败。
fn clamp(value: i32) -> i32 {
    value.clamp(-COLOR_RANGE, COLOR_RANGE)
}

/// 相对路径按工程目录解析；绝对路径原样用。
fn resolve(base_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() || base_dir.as_os_str().is_empty() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
}

/// 滤镜串里的路径：反斜杠换正斜杠、冒号与单引号转义，再整体包进单引号（调用方负责）。
///
/// 与 `clip::export::effects::filter_path` 同一套规则：两处都是把用户目录塞进
/// ffmpeg 的滤镜参数里，规则不一致就会有一边在 Windows 上炸掉。
fn filter_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .replace('\'', "\\'")
        .replace(':', "\\:")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grade(brightness: i32, contrast: i32, saturation: i32, temperature: i32) -> ColorGrade {
        ColorGrade {
            brightness,
            contrast,
            saturation,
            temperature,
            lut: None,
        }
    }

    #[test]
    fn 全默认不出滤镜() {
        let grade = ColorGrade::default();
        assert!(grade.is_neutral());
        assert_eq!(grade.filter_chain(Path::new("")), "");
        assert_eq!(grade.summary(), "原始色彩");
    }

    #[test]
    fn 亮度对比度饱和度合成一条_eq() {
        let chain = grade(50, -50, 0, 0).filter_chain(Path::new(""));
        assert_eq!(chain, "eq=brightness=0.500:contrast=0.500:saturation=1.000");
    }

    #[test]
    fn 只改色温时只有_colorbalance() {
        let chain = grade(0, 0, 0, 100).filter_chain(Path::new(""));
        assert_eq!(chain, "colorbalance=rs=0.300:bs=-0.300");
        // 冷色调反过来：红减、蓝加。
        let cool = grade(0, 0, 0, -100).filter_chain(Path::new(""));
        assert_eq!(cool, "colorbalance=rs=-0.300:bs=0.300");
    }

    #[test]
    fn 超范围的值被夹住() {
        let chain = grade(400, -400, 0, 0).filter_chain(Path::new(""));
        assert!(chain.contains("brightness=1.000"), "{chain}");
        assert!(chain.contains("contrast=0.000"), "{chain}");
        assert!(!chain.contains("2.5"), "{chain}");
    }

    #[test]
    fn lut_的_windows_路径被转义() {
        let grade = ColorGrade {
            lut: Some(PathBuf::from(r"C:\luts\cinema look.cube")),
            ..ColorGrade::default()
        };
        assert_eq!(
            grade.filter_chain(Path::new("")),
            r"lut3d='C\:/luts/cinema look.cube'"
        );
    }

    #[test]
    fn lut_的相对路径按工程目录解析() {
        let grade = ColorGrade {
            lut: Some(PathBuf::from("luts/warm.cube")),
            ..ColorGrade::default()
        };
        let chain = grade.filter_chain(Path::new("/home/me/film"));
        assert!(chain.contains("/home/me/film/luts/warm.cube"), "{chain}");
    }

    #[test]
    fn 滤镜顺序是_eq_色温_lut() {
        let grade = ColorGrade {
            brightness: 10,
            temperature: 20,
            lut: Some(PathBuf::from("/luts/a.cube")),
            ..ColorGrade::default()
        };
        let chain = grade.filter_chain(Path::new(""));
        let eq = chain.find("eq=").expect("有 eq");
        let balance = chain.find("colorbalance=").expect("有色温");
        let lut = chain.find("lut3d=").expect("有 lut");
        assert!(eq < balance && balance < lut, "{chain}");
    }

    #[test]
    fn 摘要把非零项与_lut_都说出来() {
        let grade = ColorGrade {
            brightness: 10,
            temperature: -20,
            lut: Some(PathBuf::from("/luts/a.cube")),
            ..ColorGrade::default()
        };
        let summary = grade.summary();
        assert!(summary.contains("亮度 +10"), "{summary}");
        assert!(summary.contains("色温 -20"), "{summary}");
        assert!(summary.contains("LUT a.cube"), "{summary}");
        assert!(!summary.contains("对比度"), "{summary}");
    }

    #[test]
    fn 序列往返_json_不丢字段() {
        let grade = ColorGrade {
            brightness: -30,
            contrast: 20,
            saturation: 15,
            temperature: 5,
            lut: Some(PathBuf::from("luts/a.cube")),
        };
        let text = serde_json::to_string(&grade).unwrap();
        let parsed: ColorGrade = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, grade);
    }

    #[test]
    fn 缺字段按默认值读() {
        // 老工程没有 `color`：`#[serde(default)]` 兜住，不是错误。
        let grade: ColorGrade = serde_json::from_str(r#"{"brightness": 20}"#).unwrap();
        assert_eq!(grade.brightness, 20);
        assert_eq!(grade.contrast, 0);
        assert_eq!(grade.lut, None);
    }
}
