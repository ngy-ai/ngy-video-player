//! 轨道：类型、片段 / 转场 / 字幕条目、静音与锁定、音量与独奏。
//!
//! 单独成文件是因为 `Track` 的访问器比序列本身还多（片段查找、重叠、可见区间、
//! 排序、字幕……），和 [`super::Sequence`] 放在一起会把模型文件推过 700 行。

use super::*;

/// 轨道类型。片段只能待在同类轨道上（M2-T04 换轨时按它判断兼容）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    #[default]
    Video,
    Audio,
    Subtitle,
}

impl TrackKind {
    /// 界面上的中文名，也用于自动起名。
    pub fn label(self) -> &'static str {
        match self {
            TrackKind::Video => "视频",
            TrackKind::Audio => "音频",
            TrackKind::Subtitle => "字幕",
        }
    }
}

/// 一条轨道：一组互不重叠的片段 + 它们之间的转场。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Track {
    pub id: Id,
    pub kind: TrackKind,
    pub name: String,
    pub clips: Vec<ClipItem>,
    pub transitions: Vec<Transition>,
    /// 字幕轨上的文字条目。只有 `Subtitle` 轨会有内容，其它轨恒为空。
    pub subtitles: Vec<SubtitleItem>,
    /// 这一轨不参与预览与导出：视频 / 字幕轨隐藏，音频轨静音。
    ///
    /// 用一个字段而不是 `hidden` + `muted`：轨道的 `kind` 已经说明它是画面还是声音，
    /// 两个字段只会让每个调用方多记一条「视频轨别看 muted」。
    pub muted: bool,
    /// 锁定：不影响输出，只拒绝一切编辑操作。
    pub locked: bool,
    /// 这一轨的音量：**线性增益**，1.0 = 原样。
    ///
    /// 与导出侧的 `volume` 滤镜同一个量纲（都是振幅倍数，不是 dB），预览把它乘到
    /// mpv 的 `volume` 上，两边电平才对得上。只对音频轨有意义；视频 / 字幕轨恒为 1。
    #[serde(default = "unity_gain")]
    pub gain: f32,
    /// 独奏：只要有任意一条音频轨开着独奏，其它音频轨就都不出声。
    ///
    /// 独奏**优先于静音**（与常见 DAW 一致）：一条轨既静音又独奏时它是响的——
    /// 用户点独奏的心意是「只听这条」。
    #[serde(default)]
    pub solo: bool,
}

/// 音量的默认值（原样）。`serde` 的 `default` 需要一个函数，`f32` 的默认 0.0 是
/// 「全静音」，直接 [`Default`] 会让老工程一读进来就哑掉。
fn unity_gain() -> f32 {
    1.0
}

/// 轨道音量的上限：2.0 = +6 dB。再往上放就只剩削顶的风险，混音台上也没人这么推。
pub const MAX_GAIN: f32 = 2.0;

impl Track {
    pub fn new(id: Id, kind: TrackKind, name: impl Into<String>) -> Self {
        Self {
            id,
            kind,
            name: name.into(),
            clips: Vec::new(),
            transitions: Vec::new(),
            subtitles: Vec::new(),
            muted: false,
            locked: false,
            gain: 1.0,
            solo: false,
        }
    }

    /// 可用于混音的音量：坏值（NaN / 负数 / 无穷）按 1.0，超过上限夹到 [`MAX_GAIN`]。
    ///
    /// 工程文件是明文 JSON，手改出来的 `gain` 可能是任何数；这里兜底而不是在每个
    /// 使用点上各写一遍。
    pub fn normalized_gain(&self) -> f32 {
        if self.gain.is_finite() && self.gain >= 0.0 {
            self.gain.min(MAX_GAIN)
        } else {
            1.0
        }
    }

    /// 这一刻该不该出声。`solo_on` 是「本序列里有音频轨在独奏」。
    ///
    /// 规则只有一条：有独奏时只有独奏的那几条响；没有独奏时看 `muted`。
    pub fn audible(&self, solo_on: bool) -> bool {
        if solo_on { self.solo } else { !self.muted }
    }

    /// 音量的展示文案：0 dB / -6.0 dB / +6.0 dB，0 增益写「静音」。
    ///
    /// 面板上的档位与轨道头上的小字共用它，两边不会一个写 dB 一个写百分比。
    pub fn gain_label(&self) -> String {
        let gain = self.normalized_gain();
        if (gain - 1.0).abs() < 1e-3 {
            "0 dB".to_string()
        } else if gain <= 0.0 {
            "静音".to_string()
        } else {
            format!("{:+.1} dB", 20.0 * gain.log10())
        }
    }

    pub fn clip(&self, id: Id) -> Option<&ClipItem> {
        self.clips.iter().find(|clip| clip.id == id)
    }

    pub fn clip_mut(&mut self, id: Id) -> Option<&mut ClipItem> {
        self.clips.iter_mut().find(|clip| clip.id == id)
    }

    pub fn clip_index(&self, id: Id) -> Option<usize> {
        self.clips.iter().position(|clip| clip.id == id)
    }

    /// 轨道内容的终点（片段与字幕条目里最晚的那个出点）。空轨道是 0。
    ///
    /// 字幕也要算进去：只有字幕轨上有内容时，序列时长不能还是 0，否则导出计划
    /// 会把整条序列当空的，预览播放头也走不过字幕。
    pub fn end(&self) -> Tick {
        self.clips
            .iter()
            .map(ClipItem::end)
            .chain(self.subtitles.iter().map(|item| item.end))
            .max()
            .unwrap_or(Tick::ZERO)
    }

    /// 找和 `[start, end)` 重叠的片段，跳过 `except`。
    ///
    /// 首尾正好相接（`end == clip.start`）不算重叠：两个片段可以无缝贴在一起。
    pub fn overlap(&self, start: Tick, end: Tick, except: Option<Id>) -> Option<&ClipItem> {
        self.clips
            .iter()
            .find(|clip| Some(clip.id) != except && clip.start < end && start < clip.end())
    }

    /// 按起点排序。加载工程与批量操作后调用，维持「片段有序」的不变量。
    pub fn sort_clips(&mut self) {
        self.clips.sort_by_key(|clip| (clip.start, clip.id));
    }

    /// 与 `[from, to)` 有交集的片段。
    ///
    /// `clips` 按起点有序是模型的不变量（`sort_clips`），所以先用二分跳过整段都在
    /// 视口左边的那一截：万级片段的工程每帧都从头线性扫一遍，滚动就会掉帧。
    pub fn visible_clips(&self, from: Tick, to: Tick) -> impl Iterator<Item = &ClipItem> {
        let first = self.clips.partition_point(|clip| clip.end() <= from);
        self.clips[first..]
            .iter()
            .take_while(move |clip| clip.start < to)
    }

    pub fn sort_transitions(&mut self) {
        self.transitions.sort_by_key(|transition| transition.id);
    }

    pub fn subtitle(&self, id: Id) -> Option<&SubtitleItem> {
        self.subtitles.iter().find(|item| item.id == id)
    }

    pub fn subtitle_mut(&mut self, id: Id) -> Option<&mut SubtitleItem> {
        self.subtitles.iter_mut().find(|item| item.id == id)
    }

    /// 按起点排序。字幕条目比片段少得多，加载工程时排一次就够。
    pub fn sort_subtitles(&mut self) {
        self.subtitles.sort_by_key(|item| (item.start, item.id));
    }

    /// 与 `[from, to)` 有交集的字幕条目，供时间线只画视口里的那几条。
    pub fn visible_subtitles(&self, from: Tick, to: Tick) -> impl Iterator<Item = &SubtitleItem> {
        self.subtitles
            .iter()
            .filter(move |item| item.start < to && item.end > from)
    }
}
