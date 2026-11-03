//! 导出预设：把一整套 [`ExportOptions`] 起个名字存下来，下次一键套用。
//!
//! 裁切面板里的选项默认只对这一次导出有效，关掉面板就回默认值；预设是唯一跨会话
//! 保留的那一份，存在配置目录下的 `presets.json`（见 `settings::presets_path`）。
//!
//! 读的时候逐条解析：某一条因为版本变化读不出来（比如多了个不认识的值），只丢那
//! 一条并记一行日志，不让整份预设跟着作废——用户攒下来的东西不能因为一条坏掉全没。

use super::ExportOptions;
use serde::{Deserialize, Serialize};

/// 预设名的长度上限：面板一行放不下更长的名字，配置文件里也不必存废话。
pub const MAX_NAME_CHARS: usize = 24;

/// 一份预设：名字 + 一整套导出参数。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportPreset {
    pub name: String,
    #[serde(default)]
    pub options: ExportOptions,
}

/// 已保存的全部预设，顺序就是面板上的展示顺序。
#[derive(Clone, Debug, Default)]
pub struct PresetStore {
    presets: Vec<ExportPreset>,
}

impl PresetStore {
    /// 从磁盘读。文件不存在或整份坏掉时得到一个空列表：预设不是必需数据，
    /// 读不出来时不该拦住用户导出。
    pub fn load() -> Self {
        let Some(path) = crate::settings::presets_path() else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        Self::parse(&text)
    }

    /// 解析存档。逐条反序列化，坏掉的那条跳过。
    fn parse(text: &str) -> Self {
        let values: Vec<serde_json::Value> = match serde_json::from_str(text) {
            Ok(values) => values,
            Err(error) => {
                log_warn!("预设文件解析失败，按空列表处理: {error}");
                return Self::default();
            }
        };
        let mut presets = Vec::with_capacity(values.len());
        for value in values {
            match serde_json::from_value::<ExportPreset>(value) {
                Ok(preset) => match normalize_name(&preset.name) {
                    Some(name) => presets.push(ExportPreset {
                        name,
                        options: preset.options,
                    }),
                    // 名字空掉的条目在界面上点不出来，留着只会占位置。
                    None => log_warn!("预设文件里有一条没有名字，已跳过"),
                },
                Err(error) => log_warn!("跳过一条读不出来的预设: {error}"),
            }
        }
        Self { presets }
    }

    /// 写回磁盘。先写临时文件再改名，中途崩溃不会留下半截文件。
    pub fn save(&self) {
        let Some(path) = crate::settings::presets_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let result = serde_json::to_string_pretty(&self.presets)
            .map_err(anyhow::Error::from)
            .and_then(|text| {
                let temp = path.with_extension("json.tmp");
                std::fs::write(&temp, text)?;
                std::fs::rename(&temp, &path)?;
                Ok(())
            });
        if let Err(error) = result {
            log_warn!("保存预设失败: {error:#}");
        }
    }

    pub fn presets(&self) -> &[ExportPreset] {
        &self.presets
    }

    pub fn is_empty(&self) -> bool {
        self.presets.is_empty()
    }

    /// 存一份预设；同名覆盖。
    ///
    /// 返回 `None` 表示名字是空的；`Some(true)` 表示覆盖了同名项。
    /// 只在内存里改，调用方随后要 [`PresetStore::save`]。
    pub fn upsert(&mut self, name: &str, options: &ExportOptions) -> Option<bool> {
        let name = normalize_name(name)?;
        if let Some(existing) = self.presets.iter_mut().find(|preset| preset.name == name) {
            existing.options = options.clone();
            return Some(true);
        }
        self.presets.push(ExportPreset {
            name,
            options: options.clone(),
        });
        Some(false)
    }

    /// 删掉第 `index` 条，返回是否真的删了。同样只改内存。
    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.presets.len() {
            return false;
        }
        self.presets.remove(index);
        true
    }
}

/// 规整预设名：去掉首尾空白、截到长度上限；全是空白时返回 `None`。
pub fn normalize_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(MAX_NAME_CHARS).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::{OutputFormat, Quality, Scale, Watermark};

    /// 一整套非默认值：往返一次后每个字段都要还在。
    fn sample() -> ExportOptions {
        ExportOptions {
            format: OutputFormat::Gif,
            quality: Quality::High,
            watermark: Watermark {
                enabled: true,
                text: "示例 @频道".into(),
                ..Watermark::default()
            },
            transform: crate::clip::VideoTransform {
                scale: Scale::P720,
                ..Default::default()
            },
            ..ExportOptions::default()
        }
    }

    #[test]
    fn 预设往返_json_不丢字段() {
        let mut store = PresetStore::default();
        assert_eq!(store.upsert("手机横屏", &sample()), Some(false));

        let text = serde_json::to_string(store.presets()).unwrap();
        let parsed = PresetStore::parse(&text);
        assert_eq!(parsed.presets().len(), 1);
        assert_eq!(parsed.presets()[0].name, "手机横屏");
        assert_eq!(parsed.presets()[0].options, sample());
    }

    #[test]
    fn 同名预设覆盖而不是堆积() {
        let mut store = PresetStore::default();
        assert_eq!(store.upsert("压缩", &ExportOptions::default()), Some(false));
        assert_eq!(store.upsert("压缩", &sample()), Some(true));
        assert_eq!(store.presets().len(), 1);
        assert_eq!(store.presets()[0].options, sample());
    }

    #[test]
    fn 空名字不保存() {
        let mut store = PresetStore::default();
        assert_eq!(store.upsert("   ", &ExportOptions::default()), None);
        assert!(store.is_empty());
    }

    #[test]
    fn 删除预设按序号() {
        let mut store = PresetStore::default();
        store.upsert("甲", &ExportOptions::default());
        store.upsert("乙", &ExportOptions::default());
        assert!(store.remove(0));
        assert_eq!(store.presets().len(), 1);
        assert_eq!(store.presets()[0].name, "乙");
        assert!(!store.remove(9), "越界不该 panic");
    }

    #[test]
    fn 一条读不出来只丢这条() {
        // 中间那条的 format 是未来的版本才有的值：反序列化会失败。
        let text = r#"[
            {"name": "好的", "options": {"quality": "high"}},
            {"name": "坏的", "options": {"format": "unknown_format"}},
            {"name": "后来的", "options": {}}
        ]"#;
        let store = PresetStore::parse(text);
        assert_eq!(store.presets().len(), 2);
        assert_eq!(store.presets()[0].name, "好的");
        assert_eq!(store.presets()[1].name, "后来的");
        assert_eq!(store.presets()[0].options.quality, Quality::High);
    }

    #[test]
    fn 名字截到长度上限() {
        let long = "很".repeat(MAX_NAME_CHARS + 5);
        let name = normalize_name(&long).unwrap();
        assert_eq!(name.chars().count(), MAX_NAME_CHARS);
    }

    #[test]
    fn 整份坏掉时按空列表处理() {
        let store = PresetStore::parse("这不是 json");
        assert!(store.is_empty());
    }
}
