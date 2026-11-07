//! 片段效果栈：一段画面在裁剪缩放之后、转场之前要过的滤镜。
//!
//! 效果按加入顺序串成一条链（`hqdn3d,unsharp,…`），先加的先生效——锐化要放在降噪后面
//! 才能把糊掉的边缘找回来，顺序反过来只是把噪点一起放大。面板里能上移 / 下移就是为这个。
//!
//! 只有画面效果：声音的加工（响度、淡入淡出）在导出参数里，不走这条链。
//!
//! 认不出来的效果名（老版本写的、或者手改坏的工程）不阻止打开：反序列化成
//! [`EffectKind::Unknown`]，导出与界面都跳过它，加载工程时统一提示一句。

use crate::clip::{Watermark, WatermarkPosition};
use crate::timeline::{Id, Sequence};
use serde::{Deserialize, Serialize};

/// 效果类型。存进工程的是字符串（`"denoise"`），认不出来的落进 `Unknown`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "String", from = "String")]
pub enum EffectKind {
    /// 隔行素材去梳齿。逐行素材上等于没加，留着不碍事。
    Deinterlace,
    /// 降噪：先抹平色噪再抹亮度噪，代价是细节变软。
    #[default]
    Denoise,
    /// 锐化：把边缘再提回来一点，别用太狠，会出现白边。
    Sharpen,
    /// 防抖：单遍的 `deshake`，手持抖动轻的时候够用。
    Stabilize,
    /// 插帧：`minterpolate`，很慢，只在导出时生效。
    Interpolate,
    /// 文字水印，参数与裁切面板里的水印是同一套。
    Watermark,
    /// 工程里有但这一版不认识的效果。
    Unknown,
}

impl EffectKind {
    pub const ALL: [EffectKind; 6] = [
        EffectKind::Deinterlace,
        EffectKind::Denoise,
        EffectKind::Sharpen,
        EffectKind::Stabilize,
        EffectKind::Interpolate,
        EffectKind::Watermark,
    ];

    /// 存进工程文件的字面量。
    pub fn id(self) -> &'static str {
        match self {
            EffectKind::Deinterlace => "deinterlace",
            EffectKind::Denoise => "denoise",
            EffectKind::Sharpen => "sharpen",
            EffectKind::Stabilize => "stabilize",
            EffectKind::Interpolate => "interpolate",
            EffectKind::Watermark => "watermark",
            EffectKind::Unknown => "unknown",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            EffectKind::Deinterlace => "去隔行",
            EffectKind::Denoise => "降噪",
            EffectKind::Sharpen => "锐化",
            EffectKind::Stabilize => "防抖",
            EffectKind::Interpolate => "插帧",
            EffectKind::Watermark => "水印",
            EffectKind::Unknown => "未知效果",
        }
    }

    fn from_id(id: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|kind| kind.id() == id)
            .unwrap_or(EffectKind::Unknown)
    }

    /// 这类效果用哪个参数：面板据此决定显示「强度」还是「倍数」。
    pub fn knob(self) -> Knob {
        match self {
            EffectKind::Interpolate => Knob::Factor,
            EffectKind::Watermark => Knob::Text,
            EffectKind::Deinterlace => Knob::None,
            _ => Knob::Strength,
        }
    }
}

impl From<EffectKind> for String {
    fn from(kind: EffectKind) -> Self {
        kind.id().to_string()
    }
}

impl From<String> for EffectKind {
    fn from(id: String) -> Self {
        EffectKind::from_id(&id)
    }
}

/// 面板上这一行能调什么。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Knob {
    None,
    Strength,
    Factor,
    Text,
}

/// 效果参数。各类效果共用一个结构：字段少，分开存反而让工程文件里出现
/// 一堆只有某一种效果才看得懂的键。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EffectParams {
    /// 强度 1–3（弱 / 中 / 强），降噪、锐化、防抖用它。
    pub strength: u8,
    /// 插帧倍数 2–4。
    pub factor: u8,
    /// 水印文字。
    pub text: String,
    pub position: WatermarkPosition,
    pub size: u32,
    pub opacity: f32,
}

impl Default for EffectParams {
    fn default() -> Self {
        Self {
            strength: 2,
            factor: 2,
            text: String::new(),
            position: WatermarkPosition::default(),
            size: 28,
            opacity: 0.7,
        }
    }
}

impl EffectParams {
    pub const MIN_STRENGTH: u8 = 1;
    pub const MAX_STRENGTH: u8 = 3;
    pub const MIN_FACTOR: u8 = 2;
    pub const MAX_FACTOR: u8 = 4;

    /// 强度档位的中文名。
    pub fn strength_label(&self) -> &'static str {
        match self.strength.clamp(Self::MIN_STRENGTH, Self::MAX_STRENGTH) {
            1 => "弱",
            3 => "强",
            _ => "中",
        }
    }
}

/// 挂在片段上的一个效果。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Effect {
    pub id: Id,
    pub kind: EffectKind,
    /// 关掉的效果留在列表里但不出现在导出命令里：试效果时来回开关比删了重加省事。
    pub enabled: bool,
    pub params: EffectParams,
}

impl Default for Effect {
    fn default() -> Self {
        Self {
            id: Id::default(),
            kind: EffectKind::default(),
            enabled: true,
            params: EffectParams::default(),
        }
    }
}

impl Effect {
    pub fn new(id: Id, kind: EffectKind) -> Self {
        Self {
            id,
            kind,
            enabled: true,
            params: EffectParams::default(),
        }
    }

    /// 会不会出现在导出的滤镜链里。
    pub fn active(&self) -> bool {
        self.enabled && self.kind != EffectKind::Unknown && self.filter(30.0, "片段").is_some()
    }

    /// 这一条效果的滤镜串（不含前后的逗号）；不生效时返回 `None`。
    ///
    /// `fps` 是序列帧率，只有插帧用得上：目标帧率 = 序列帧率 × 倍数。
    /// `fallback` 是水印没写字时用的文字（片段名）：面板上没有输入框，
    /// 打的是这一段自己的名字，改文字留给后续的字幕 / 文字图层。
    pub fn filter(&self, fps: f64, fallback: &str) -> Option<String> {
        if !self.enabled {
            return None;
        }
        let strength = self
            .params
            .strength
            .clamp(EffectParams::MIN_STRENGTH, EffectParams::MAX_STRENGTH);
        let filter = match self.kind {
            EffectKind::Deinterlace => "yadif=mode=send_frame:parity=auto:deint=all".to_string(),
            EffectKind::Denoise => {
                let depth = match strength {
                    1 => "2:1:2:3",
                    3 => "4:3:4:5",
                    _ => "3:2:3:4",
                };
                format!("hqdn3d={depth}")
            }
            EffectKind::Sharpen => {
                let amount = match strength {
                    1 => 0.6,
                    3 => 1.6,
                    _ => 1.0,
                };
                format!("unsharp=5:5:{amount:.2}")
            }
            EffectKind::Stabilize => {
                let radius = match strength {
                    1 => 16,
                    3 => 48,
                    _ => 32,
                };
                format!("deshake=rx={radius}:ry={radius}")
            }
            EffectKind::Interpolate => {
                let factor = self
                    .params
                    .factor
                    .clamp(EffectParams::MIN_FACTOR, EffectParams::MAX_FACTOR);
                // 目标是「每秒多少帧」，而序列帧率是分数：先算出浮点再交给 ffmpeg。
                let target = fps.max(1.0) * f64::from(factor);
                format!("minterpolate=fps={target:.3}:mi_mode=mci:mc_mode=aobmc:vsbmc=1")
            }
            EffectKind::Watermark => {
                let own = self.params.text.trim();
                let text = if own.is_empty() { fallback.trim() } else { own };
                if text.is_empty() {
                    return None;
                }
                // 文字滤镜只有一份实现，水印参数借用裁切面板那套结构。
                Watermark {
                    enabled: true,
                    text: text.to_string(),
                    position: self.params.position,
                    size: self.params.size,
                    opacity: self.params.opacity,
                }
                .filter()?
            }
            EffectKind::Unknown => return None,
        };
        Some(filter)
    }

    /// 面板上的一行标题：类型 + 当前参数。
    pub fn describe(&self) -> String {
        match self.kind {
            EffectKind::Denoise | EffectKind::Sharpen | EffectKind::Stabilize => {
                format!("{} · {}", self.kind.label(), self.params.strength_label())
            }
            EffectKind::Interpolate => {
                let factor = self
                    .params
                    .factor
                    .clamp(EffectParams::MIN_FACTOR, EffectParams::MAX_FACTOR);
                format!("插帧 · {factor}×")
            }
            EffectKind::Watermark => {
                let text = self.params.text.trim();
                if text.is_empty() {
                    "水印（还没写字）".to_string()
                } else {
                    format!("水印「{text}」")
                }
            }
            other => other.label().to_string(),
        }
    }
}

/// 片段上按顺序生效的滤镜串，直接拼进 `filter_complex`（前面自带逗号）。
///
/// 一条都没有时返回空串：调用方不用判断。
pub fn filter_chain(sequence: &Sequence, clip: Id) -> String {
    let Some(item) = sequence.clip(clip) else {
        return String::new();
    };
    let fps = sequence_fps(sequence);
    let mut chain = String::new();
    for effect in &item.effects {
        if let Some(filter) = effect.filter(fps, &item.name) {
            chain.push(',');
            chain.push_str(&filter);
        }
    }
    chain
}

/// 追加一个效果，返回它的 id。片段不存在时返回 `None`。
pub fn add_effect(sequence: &mut Sequence, clip: Id, kind: EffectKind) -> Option<Id> {
    let id = sequence.alloc_id();
    let item = sequence.clip_mut(clip)?;
    item.effects.push(Effect::new(id, kind));
    Some(id)
}

/// 删掉一个效果。
pub fn remove_effect(sequence: &mut Sequence, clip: Id, effect: Id) -> bool {
    let Some(item) = sequence.clip_mut(clip) else {
        return false;
    };
    let before = item.effects.len();
    item.effects.retain(|item| item.id != effect);
    item.effects.len() != before
}

/// 上移 / 下移一位：链上的先后决定结果，顺序本身是编辑的一部分。
pub fn move_effect(sequence: &mut Sequence, clip: Id, effect: Id, up: bool) -> bool {
    let Some(item) = sequence.clip_mut(clip) else {
        return false;
    };
    let Some(index) = item.effects.iter().position(|item| item.id == effect) else {
        return false;
    };
    let target = if up {
        index.checked_sub(1)
    } else {
        (index + 1 < item.effects.len()).then_some(index + 1)
    };
    let Some(target) = target else {
        return false;
    };
    item.effects.swap(index, target);
    true
}

/// 开关一个效果，返回它现在是否开着。
pub fn toggle_effect(sequence: &mut Sequence, clip: Id, effect: Id) -> Option<bool> {
    let item = sequence
        .clip_mut(clip)?
        .effects
        .iter_mut()
        .find(|item| item.id == effect)?;
    item.enabled = !item.enabled;
    Some(item.enabled)
}

/// 强度 / 倍数循环到下一档（弱 → 中 → 强 → 弱）。
pub fn cycle_knob(sequence: &mut Sequence, clip: Id, effect: Id) -> bool {
    let Some(item) = sequence
        .clip_mut(clip)
        .and_then(|clip| clip.effects.iter_mut().find(|item| item.id == effect))
    else {
        return false;
    };
    match item.kind.knob() {
        Knob::Strength => {
            let next = if item.params.strength >= EffectParams::MAX_STRENGTH {
                EffectParams::MIN_STRENGTH
            } else {
                item.params.strength + 1
            };
            item.params.strength = next;
        }
        Knob::Factor => {
            let next = if item.params.factor >= EffectParams::MAX_FACTOR {
                EffectParams::MIN_FACTOR
            } else {
                item.params.factor + 1
            };
            item.params.factor = next;
        }
        // 去隔行没有参数，水印的文字要打字，不用循环。
        Knob::None | Knob::Text => return false,
    }
    true
}

/// 工程里认不出来的效果有几个：加载时提示一句，别让人以为效果丢了。
pub fn unknown_effects(sequence: &Sequence) -> usize {
    sequence
        .clips()
        .flat_map(|(_, clip)| clip.effects.iter())
        .filter(|effect| effect.kind == EffectKind::Unknown)
        .count()
}

/// 序列帧率的浮点值。
pub fn sequence_fps(sequence: &Sequence) -> f64 {
    if sequence.fps.den() == 0 {
        return 0.0;
    }
    sequence.fps.num() as f64 / f64::from(sequence.fps.den())
}

/// 插帧目标帧率（给面板提示用）。
pub fn interpolated_fps(sequence: &Sequence, factor: u8) -> f64 {
    sequence_fps(sequence)
        * f64::from(factor.clamp(EffectParams::MIN_FACTOR, EffectParams::MAX_FACTOR))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{ClipItem, Rational, Sequence, Tick, TrackKind};

    fn sequence() -> (Sequence, Id) {
        let mut sequence = Sequence::new(
            "测试",
            Rational::new(30, 1).expect("30 fps 合法"),
            1920,
            1080,
        );
        sequence.add_track(TrackKind::Video);
        let track = sequence.tracks[0].id;
        let id = sequence.alloc_id();
        sequence.track_mut(track).unwrap().clips.push(ClipItem::new(
            id,
            crate::timeline::AssetId::new(1),
            Tick::ZERO,
            Tick::from_micros(2_000_000),
            Tick::ZERO,
        ));
        (sequence, id)
    }

    #[test]
    fn 按加入顺序拼滤镜链() {
        let (mut sequence, clip) = sequence();
        add_effect(&mut sequence, clip, EffectKind::Denoise).unwrap();
        add_effect(&mut sequence, clip, EffectKind::Sharpen).unwrap();

        let chain = filter_chain(&sequence, clip);
        let denoise = chain.find("hqdn3d").expect("有降噪");
        let sharpen = chain.find("unsharp").expect("有锐化");
        assert!(denoise < sharpen, "先加的先生效：{chain}");
        assert!(chain.starts_with(','), "链首自带逗号：{chain}");
    }

    #[test]
    fn 关掉的效果不进链() {
        let (mut sequence, clip) = sequence();
        let id = add_effect(&mut sequence, clip, EffectKind::Sharpen).unwrap();
        toggle_effect(&mut sequence, clip, id);
        assert_eq!(filter_chain(&sequence, clip), "");
    }

    #[test]
    fn 未知效果不产生滤镜也不阻塞别的效果() {
        let (mut sequence, clip) = sequence();
        add_effect(&mut sequence, clip, EffectKind::Unknown).unwrap();
        add_effect(&mut sequence, clip, EffectKind::Deinterlace).unwrap();
        assert_eq!(unknown_effects(&sequence), 1);
        let chain = filter_chain(&sequence, clip);
        assert!(chain.contains("yadif"), "{chain}");
        assert!(!chain.contains("unknown"), "{chain}");
    }

    #[test]
    fn 强度决定滤镜参数() {
        let (mut sequence, clip) = sequence();
        let id = add_effect(&mut sequence, clip, EffectKind::Sharpen).unwrap();
        let weak = filter_chain(&sequence, clip);
        assert!(weak.contains("unsharp=5:5:1.00"), "{weak}");

        // 1 → 2 → 3，3 之后回到 1。
        cycle_knob(&mut sequence, clip, id);
        assert!(filter_chain(&sequence, clip).contains("unsharp=5:5:1.60"));
        cycle_knob(&mut sequence, clip, id);
        assert!(filter_chain(&sequence, clip).contains("unsharp=5:5:0.60"));
        assert!(
            !move_effect(&mut sequence, clip, id, true),
            "只有一条时移不动"
        );
    }

    #[test]
    fn 插帧目标帧率按序列帧率翻倍() {
        let (mut sequence, clip) = sequence();
        add_effect(&mut sequence, clip, EffectKind::Interpolate).unwrap();
        let chain = filter_chain(&sequence, clip);
        assert!(chain.contains("minterpolate=fps=60.000"), "{chain}");
        assert_eq!(interpolated_fps(&sequence, 3), 90.0);
    }

    #[test]
    fn 水印没写字时用片段名() {
        let (mut sequence, clip) = sequence();
        add_effect(&mut sequence, clip, EffectKind::Watermark).unwrap();
        assert_eq!(filter_chain(&sequence, clip), "", "没字也没名就不画");

        sequence.clip_mut(clip).unwrap().name = "开场".into();
        let chain = filter_chain(&sequence, clip);
        assert!(chain.contains("drawtext="), "{chain}");
        assert!(chain.contains("text='开场'"), "{chain}");

        // 自己写了字就用自己的，片段名只是兜底。
        let effect = sequence
            .clip_mut(clip)
            .unwrap()
            .effects
            .iter_mut()
            .find(|effect| effect.kind == EffectKind::Watermark)
            .unwrap();
        effect.params.text = "示例".into();
        let chain = filter_chain(&sequence, clip);
        assert!(chain.contains("text='示例'"), "{chain}");
    }

    #[test]
    fn 效果能上移下移() {
        let (mut sequence, clip) = sequence();
        let denoise = add_effect(&mut sequence, clip, EffectKind::Denoise).unwrap();
        let sharpen = add_effect(&mut sequence, clip, EffectKind::Sharpen).unwrap();

        assert!(move_effect(&mut sequence, clip, sharpen, true));
        let order: Vec<Id> = sequence
            .clip(clip)
            .unwrap()
            .effects
            .iter()
            .map(|effect| effect.id)
            .collect();
        assert_eq!(order, vec![sharpen, denoise]);

        assert!(
            !move_effect(&mut sequence, clip, sharpen, true),
            "已经在最前"
        );
        assert!(move_effect(&mut sequence, clip, sharpen, false));
    }

    #[test]
    fn 删掉的效果不再出现() {
        let (mut sequence, clip) = sequence();
        let id = add_effect(&mut sequence, clip, EffectKind::Denoise).unwrap();
        assert!(remove_effect(&mut sequence, clip, id));
        assert!(!remove_effect(&mut sequence, clip, id));
        assert_eq!(filter_chain(&sequence, clip), "");
    }

    #[test]
    fn 认不出来的效果名落进未知() {
        let kind: EffectKind = "sepia".to_string().into();
        assert_eq!(kind, EffectKind::Unknown);
        let known: EffectKind = "sharpen".to_string().into();
        assert_eq!(known, EffectKind::Sharpen);
        assert_eq!(String::from(EffectKind::Sharpen), "sharpen");
    }
}
