//! 导出预设：把一套 [`ExportSettings`] 起个名字存下来，下次一键套用。
//!
//! 存在配置目录下的 `render-presets.json`（见 `settings::render_presets_path`），
//! 与裁切面板的 `presets.json` 分开：两份预设描述的是不同的东西（片段裁切的输出
//! 参数 vs 整条序列的编码参数），混在一份文件里改一边会连带另一边。
//!
//! 读的时候逐条解析：某一条因为版本变化读不出来只丢那一条并记一行日志。预设是
//! 用户攒出来的东西，不能因为一条坏掉全没。字段缺失、未知编码器名都走
//! [`ExportSettings`] 的默认值与降级规则，不会被当成「坏条目」。

use serde::{Deserialize, Serialize};

use super::settings::ExportSettings;

/// 预设名的长度上限：面板一行放不下更长的名字。
pub const PRESET_NAME_CHARS: usize = 24;

/// 一份导出预设：名字 + 一整套编码参数。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderPreset {
    pub name: String,
    #[serde(default)]
    pub settings: ExportSettings,
}

/// 已保存的全部预设，顺序就是面板上的展示顺序。
#[derive(Clone, Debug, Default)]
pub struct RenderPresetStore {
    presets: Vec<RenderPreset>,
}

impl RenderPresetStore {
    /// 从磁盘读。文件不存在或整份坏掉时得到一个空列表：预设不是必需数据，
    /// 读不出来时不该拦住用户导出。
    pub fn load() -> Self {
        let Some(path) = crate::settings::render_presets_path() else {
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
                log_warn!("导出预设文件解析失败，按空列表处理: {error}");
                return Self::default();
            }
        };
        let mut presets = Vec::with_capacity(values.len());
        for value in values {
            match serde_json::from_value::<RenderPreset>(value) {
                Ok(preset) => match normalize_name(&preset.name) {
                    Some(name) => presets.push(RenderPreset {
                        name,
                        settings: preset.settings,
                    }),
                    // 名字空掉的条目在界面上点不出来，留着只会占位置。
                    None => log_warn!("导出预设里有一条没有名字，已跳过"),
                },
                Err(error) => log_warn!("跳过一条读不出来的导出预设: {error}"),
            }
        }
        Self { presets }
    }

    /// 写回磁盘。先写临时文件再改名，中途崩溃不会留下半截文件。
    pub fn save(&self) {
        let Some(path) = crate::settings::render_presets_path() else {
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
            log_warn!("保存导出预设失败: {error:#}");
        }
    }

    pub fn presets(&self) -> &[RenderPreset] {
        &self.presets
    }

    pub fn is_empty(&self) -> bool {
        self.presets.is_empty()
    }

    /// 存一份预设；同名覆盖。
    ///
    /// 返回 `None` 表示名字是空的；`Some(true)` 表示覆盖了同名项。
    /// 只在内存里改，调用方随后要 [`RenderPresetStore::save`]。
    pub fn upsert(&mut self, name: &str, settings: &ExportSettings) -> Option<bool> {
        let name = normalize_name(name)?;
        if let Some(existing) = self.presets.iter_mut().find(|preset| preset.name == name) {
            existing.settings = settings.clone();
            return Some(true);
        }
        self.presets.push(RenderPreset {
            name,
            settings: settings.clone(),
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

    /// 当前参数是否正好等于某份预设。返回它的名字。
    ///
    /// 界面用它给预设 chip 点选中态，队列项也用它记「这一条用的是哪个预设」：
    /// 改过任何一项之后自然对不上，变成「自定义」。
    pub fn find_match(&self, settings: &ExportSettings) -> Option<&str> {
        self.presets
            .iter()
            .find(|preset| &preset.settings == settings)
            .map(|preset| preset.name.as_str())
    }
}

/// 规整预设名：去掉首尾空白、截到长度上限；全是空白时返回 `None`。
///
/// 界面保存预设时也用它：提示里说的名字必须和真正写进文件的名字一致。
pub fn normalize_name(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(PRESET_NAME_CHARS).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::export::{EncodeSpeed, Quality, VideoCodec};

    /// 一整套非默认值：往返一次后每个字段都要还在。
    fn sample() -> ExportSettings {
        ExportSettings {
            codec: VideoCodec::H265,
            quality: Quality::High,
            speed: EncodeSpeed::Slow,
            audio_bitrate: 256,
            faststart: false,
        }
    }

    #[test]
    fn 预设往返_json_不丢字段() {
        let mut store = RenderPresetStore::default();
        assert_eq!(store.upsert("交付版", &sample()), Some(false));

        let text = serde_json::to_string(store.presets()).unwrap();
        let parsed = RenderPresetStore::parse(&text);
        assert_eq!(parsed.presets().len(), 1);
        assert_eq!(parsed.presets()[0].name, "交付版");
        assert_eq!(parsed.presets()[0].settings, sample());
    }

    #[test]
    fn 同名预设覆盖而不是堆积() {
        let mut store = RenderPresetStore::default();
        let default = ExportSettings::default();
        assert_eq!(store.upsert("压缩", &default), Some(false));
        assert_eq!(store.upsert("压缩", &sample()), Some(true));
        assert_eq!(store.presets().len(), 1);
        assert_eq!(store.presets()[0].settings, sample());
    }

    #[test]
    fn 空名字不保存() {
        let mut store = RenderPresetStore::default();
        assert_eq!(store.upsert("   ", &ExportSettings::default()), None);
        assert!(store.is_empty());
    }

    #[test]
    fn 删除预设按序号() {
        let mut store = RenderPresetStore::default();
        store.upsert("甲", &ExportSettings::default());
        store.upsert("乙", &ExportSettings::default());
        assert!(store.remove(0));
        assert_eq!(store.presets().len(), 1);
        assert_eq!(store.presets()[0].name, "乙");
        assert!(!store.remove(9), "越界不该 panic");
    }

    #[test]
    fn 一条读不出来只丢这条() {
        // 中间那条的 settings 不是对象（手改坏了）：只有它读不出来。
        let text = r#"[
            {"name": "好的", "settings": {"quality": "high"}},
            {"name": "坏的", "settings": 42},
            {"name": "后来的"}
        ]"#;
        let store = RenderPresetStore::parse(text);
        assert_eq!(store.presets().len(), 2);
        assert_eq!(store.presets()[0].name, "好的");
        assert_eq!(store.presets()[0].settings.quality, Quality::High);
        assert_eq!(store.presets()[1].name, "后来的");
        assert_eq!(store.presets()[1].settings, ExportSettings::default());
    }

    #[test]
    fn 缺少_settings_的条目按默认参数读() {
        // 老文件只存了名字：不能因为少了参数就整条丢掉。
        let store = RenderPresetStore::parse(r#"[{"name": "老预设"}]"#);
        assert_eq!(store.presets().len(), 1);
        assert_eq!(store.presets()[0].settings, ExportSettings::default());
    }

    #[test]
    fn 名字截到长度上限() {
        let long = "很".repeat(PRESET_NAME_CHARS + 5);
        let name = normalize_name(&long).unwrap();
        assert_eq!(name.chars().count(), PRESET_NAME_CHARS);
    }

    #[test]
    fn 整份坏掉时按空列表处理() {
        assert!(RenderPresetStore::parse("这不是 json").is_empty());
    }

    #[test]
    fn 参数完全一致才算用着某个预设() {
        let mut store = RenderPresetStore::default();
        store.upsert("交付版", &sample());
        assert_eq!(store.find_match(&sample()), Some("交付版"));

        // 改一个字段就对不上了：界面据此熄灭选中态，队列项记为「自定义」。
        let tweaked = ExportSettings {
            audio_bitrate: 128,
            ..sample()
        };
        assert_eq!(store.find_match(&tweaked), None);
        assert_eq!(store.find_match(&ExportSettings::default()), None);
    }
}
