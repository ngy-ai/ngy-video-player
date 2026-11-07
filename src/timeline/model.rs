//! 序列模型：序列 → 轨道（视频/音频/字幕）→ 片段，外加转场与标记。
//!
//! 纯数据，不含渲染与坐标换算，也不持有文件路径：片段只记 [`AssetId`]，素材表与
//! 文件路径在工程文件里（M1-T07 登记）。id 由 [`Sequence::alloc_id`] 统一分配并随
//! 工程存盘，重开工程后保持稳定，撤销栈与界面选中都靠它认人。
//!
//! 不变量（编辑操作必须维持，加载工程时由 [`Sequence::validate`] 检查）：
//!
//! 1. 同一条轨道上的片段按起点排序、互不重叠；
//! 2. 片段时长恒为正（`source_out > source_in`）；
//! 3. 序列内所有 id 唯一。

use crate::timeline::{Rational, Tick};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;

/// 工程内实体的 id。轨道、片段、转场、标记共用一个 id 空间。
///
/// 由序列的计数器分配而不是随机生成：工程文件是明文 JSON，顺序增长的 id
/// 读起来、diff 起来都比一串随机数清楚，重开工程后也不会变。
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Id(u64);

impl Id {
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// 素材 id：指向工程素材表里的一个文件。片段不直接存路径，素材重链只改一处。
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct AssetId(u64);

impl AssetId {
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

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

/// 轨道上的一个片段。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipItem {
    pub id: Id,
    pub asset_id: AssetId,
    /// 素材入点：这一段从素材的第几微秒开始。
    pub source_in: Tick,
    /// 素材出点（不含）：`source_out - source_in` 就是片段时长。
    pub source_out: Tick,
    /// 序列上的起点。
    pub start: Tick,
    /// 显示名。留空时界面退回素材名，素材改名不会连带改这里。
    pub name: String,
    /// 效果栈：按顺序串进这条片段的滤镜链（见 [`crate::timeline::effect`]）。
    pub effects: Vec<crate::timeline::Effect>,
    /// 关键帧：缩放 / 位置 / 不透明度随时间变化的曲线（见 [`crate::timeline::keyframe`]）。
    pub keys: crate::timeline::Keyframes,
}

impl ClipItem {
    pub fn new(id: Id, asset_id: AssetId, source_in: Tick, source_out: Tick, start: Tick) -> Self {
        Self {
            id,
            asset_id,
            source_in,
            source_out,
            start,
            name: String::new(),
            effects: Vec::new(),
            keys: crate::timeline::Keyframes::default(),
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// 片段时长。模型里恒为正；读到坏数据时贴到 0，不让负时长流进界面。
    pub fn duration(&self) -> Tick {
        (self.source_out - self.source_in).max(Tick::ZERO)
    }

    /// 片段在序列上的终点（不含）。
    pub fn end(&self) -> Tick {
        self.start + self.duration()
    }

    /// 序列上的某个时刻是否落在本片段内。
    pub fn covers(&self, at: Tick) -> bool {
        at >= self.start && at < self.end()
    }

    /// 素材时间 → 序列时间。片段不覆盖该时刻时返回 `None`。
    pub fn timeline_at(&self, source: Tick) -> Option<Tick> {
        (source >= self.source_in && source < self.source_out)
            .then(|| self.start + (source - self.source_in))
    }

    /// 序列时间 → 素材时间。片段不覆盖该时刻时返回 `None`。
    pub fn source_at(&self, at: Tick) -> Option<Tick> {
        self.covers(at).then(|| self.source_in + (at - self.start))
    }
}

/// 转场类型，取值来自 ffmpeg `xfade` 的可用集合（M4-T03 接入导出）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    #[default]
    CrossDissolve,
    Fade,
    Wipe,
    Slide,
}

impl TransitionKind {
    /// 界面上的选项，顺序固定：菜单与面板照这个顺序排，用户不用每次都找。
    pub const ALL: [TransitionKind; 4] = [
        TransitionKind::CrossDissolve,
        TransitionKind::Fade,
        TransitionKind::Wipe,
        TransitionKind::Slide,
    ];

    /// 给人看的中文名。`xfade` 的滤镜名不直接拿来当界面文案。
    pub fn label(self) -> &'static str {
        match self {
            TransitionKind::CrossDissolve => "交叉溶解",
            TransitionKind::Fade => "淡入淡出",
            TransitionKind::Wipe => "擦除",
            TransitionKind::Slide => "滑动",
        }
    }
}

/// 两个相邻片段之间的转场。
///
/// 转场占用两段的重叠区间，**不改变序列总时长**：导入导出时按 `duration`
/// 分别从两侧多取一段素材。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Transition {
    pub id: Id,
    /// 时间线上在前的片段。
    pub before: Id,
    /// 时间线上在后的片段。
    pub after: Id,
    pub kind: TransitionKind,
    pub duration: Tick,
}

impl Transition {
    pub fn new(id: Id, before: Id, after: Id, kind: TransitionKind, duration: Tick) -> Self {
        Self {
            id,
            before,
            after,
            kind,
            duration,
        }
    }
}

/// 时间线标记。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Marker {
    pub id: Id,
    pub at: Tick,
    pub name: String,
}

impl Marker {
    pub fn new(id: Id, at: Tick, name: impl Into<String>) -> Self {
        Self {
            id,
            at,
            name: name.into(),
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
    /// 这一轨不参与预览与导出：视频 / 字幕轨隐藏，音频轨静音。
    ///
    /// 用一个字段而不是 `hidden` + `muted`：轨道的 `kind` 已经说明它是画面还是声音，
    /// 两个字段只会让每个调用方多记一条「视频轨别看 muted」。
    pub muted: bool,
    /// 锁定：不影响输出，只拒绝一切编辑操作。
    pub locked: bool,
}

impl Track {
    pub fn new(id: Id, kind: TrackKind, name: impl Into<String>) -> Self {
        Self {
            id,
            kind,
            name: name.into(),
            clips: Vec::new(),
            transitions: Vec::new(),
            muted: false,
            locked: false,
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

    /// 轨道内容的终点（最后一个片段的出点）。空轨道是 0。
    pub fn end(&self) -> Tick {
        self.clips
            .iter()
            .map(ClipItem::end)
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
}

/// 一条时间线。一个工程可以有多条序列（M6-T03 导出队列按序列排队）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sequence {
    pub name: String,
    /// 序列帧率。素材帧率可以不同，换算只在 `timeline::time` 里发生。
    pub fps: Rational,
    pub width: u32,
    pub height: u32,
    pub tracks: Vec<Track>,
    pub markers: Vec<Marker>,
    /// 下一个可用的 id。存盘时一起写，重开工程后新片段不会和旧 id 撞车。
    next_id: u64,
}

impl Default for Sequence {
    fn default() -> Self {
        Self {
            name: "序列 1".to_string(),
            fps: Rational::FPS_30,
            width: 1920,
            height: 1080,
            tracks: Vec::new(),
            markers: Vec::new(),
            next_id: 1,
        }
    }
}

impl Sequence {
    /// 新建一条带视频轨与音频轨的空序列。
    ///
    /// 新序列总得有地方放片段，所以默认给一条视频轨、一条音频轨；这不是「智能轨道」，
    /// 拖入时自动选轨是 M2-T04 的事。
    pub fn new(name: impl Into<String>, fps: Rational, width: u32, height: u32) -> Self {
        let mut sequence = Self {
            name: name.into(),
            fps,
            width,
            height,
            ..Self::default()
        };
        sequence.add_track(TrackKind::Video);
        sequence.add_track(TrackKind::Audio);
        sequence
    }

    /// 分配一个新 id。计数从 1 起，读到的坏值（0）也当 1 处理。
    pub fn alloc_id(&mut self) -> Id {
        let raw = self.next_id.max(1);
        self.next_id = raw.saturating_add(1);
        Id(raw)
    }

    /// 追加一条轨道，名字按同类型的序号自动起。返回新轨道 id。
    pub fn add_track(&mut self, kind: TrackKind) -> Id {
        let id = self.alloc_id();
        let index = self
            .tracks
            .iter()
            .filter(|track| track.kind == kind)
            .count()
            + 1;
        let name = format!("{} {index}", kind.label());
        self.tracks.push(Track::new(id, kind, name));
        id
    }

    pub fn track(&self, id: Id) -> Option<&Track> {
        self.tracks.iter().find(|track| track.id == id)
    }

    pub fn track_mut(&mut self, id: Id) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|track| track.id == id)
    }

    pub fn track_index(&self, id: Id) -> Option<usize> {
        self.tracks.iter().position(|track| track.id == id)
    }

    /// 全部片段，附带所在轨道。界面画时间线时按轨道分组，先拿到轨道再遍历。
    pub fn clips(&self) -> impl Iterator<Item = (&Track, &ClipItem)> {
        self.tracks
            .iter()
            .flat_map(|track| track.clips.iter().map(move |clip| (track, clip)))
    }

    pub fn clip(&self, id: Id) -> Option<&ClipItem> {
        self.tracks.iter().find_map(|track| track.clip(id))
    }

    pub fn clip_mut(&mut self, id: Id) -> Option<&mut ClipItem> {
        self.tracks.iter_mut().find_map(|track| track.clip_mut(id))
    }

    /// 片段所在的轨道 id。
    pub fn clip_track(&self, id: Id) -> Option<Id> {
        self.tracks
            .iter()
            .find(|track| track.clip(id).is_some())
            .map(|track| track.id)
    }

    /// 序列时长：所有轨道终点的最大值。
    pub fn duration(&self) -> Tick {
        self.tracks
            .iter()
            .map(Track::end)
            .max()
            .unwrap_or(Tick::ZERO)
    }

    /// 在 `at` 处加一个标记。标记按时间排序，同一时刻的排在一起。
    pub fn add_marker(&mut self, at: Tick, name: impl Into<String>) -> Id {
        let id = self.alloc_id();
        self.markers.push(Marker::new(id, at, name));
        self.markers.sort_by_key(|marker| (marker.at, marker.id));
        id
    }

    pub fn marker(&self, id: Id) -> Option<&Marker> {
        self.markers.iter().find(|marker| marker.id == id)
    }

    /// 在 `at` 处加一个标记；`tolerance` 范围内已经有标记就删掉它。返回是否新增。
    ///
    /// 不能用精确的时间相等判断：播放头是拖回来的，鼠标只精确到屏幕像素，
    /// 同一个视图位置换算出来的 `Tick` 差几十毫秒是常态，精确比较等于永远删不掉
    /// 旧标记、只会一直往上叠。容差由界面按当前缩放给出（一像素对应的时间），
    /// 这里保持纯数据判断。命中多个时删最近的那一个。
    pub fn toggle_marker(&mut self, at: Tick, name: impl Into<String>, tolerance: Tick) -> bool {
        let nearest = self
            .markers
            .iter()
            .enumerate()
            .filter(|(_, marker)| marker.at.within(at, tolerance))
            .min_by_key(|(_, marker)| at.micros().abs_diff(marker.at.micros()))
            .map(|(index, _)| index);

        if let Some(index) = nearest {
            self.markers.remove(index);
            return false;
        }
        self.add_marker(at, name);
        true
    }

    /// id 是否已经被占用（读工程时用来修 `next_id`）。
    pub fn id_is_used(&self, id: Id) -> bool {
        self.tracks.iter().any(|track| {
            track.id == id
                || track.clips.iter().any(|clip| clip.id == id)
                || track
                    .transitions
                    .iter()
                    .any(|transition| transition.id == id)
        }) || self.markers.iter().any(|marker| marker.id == id)
    }

    /// 把 `next_id` 推到一个安全位置：工程文件被手改过、id 杂乱时，
    /// 新分配的 id 仍然不会撞上已有实体。
    pub fn repair_next_id(&mut self) {
        let max = self
            .tracks
            .iter()
            .flat_map(|track| {
                std::iter::once(track.id.raw())
                    .chain(track.clips.iter().map(|clip| clip.id.raw()))
                    .chain(
                        track
                            .transitions
                            .iter()
                            .map(|transition| transition.id.raw()),
                    )
            })
            .chain(self.markers.iter().map(|marker| marker.id.raw()))
            .max()
            .unwrap_or(0);
        self.next_id = self.next_id.max(max.saturating_add(1)).max(1);
    }

    /// 检查模型不变量。加载工程、批量操作之后跑一遍；编辑操作本身就该保证它成立。
    pub fn validate(&self) -> Result<(), ModelError> {
        let mut used = HashSet::new();
        for track in &self.tracks {
            if !used.insert(track.id) {
                return Err(ModelError::DuplicateId(track.id));
            }
            for transition in &track.transitions {
                if !used.insert(transition.id) {
                    return Err(ModelError::DuplicateId(transition.id));
                }
                let before = track.clip(transition.before);
                let after = track.clip(transition.after);
                match (before, after) {
                    (Some(before), Some(after)) if before.start < after.start => {}
                    _ => return Err(ModelError::DanglingTransition(transition.id)),
                }
            }
            let mut previous_end: Option<Tick> = None;
            for clip in &track.clips {
                if !used.insert(clip.id) {
                    return Err(ModelError::DuplicateId(clip.id));
                }
                if clip.source_out <= clip.source_in || clip.start.is_negative() {
                    return Err(ModelError::EmptyClip(clip.id));
                }
                if let Some(end) = previous_end
                    && clip.start < end
                {
                    return Err(ModelError::Overlap(track.id));
                }
                previous_end = Some(clip.end());
            }
        }
        for marker in &self.markers {
            if !used.insert(marker.id) {
                return Err(ModelError::DuplicateId(marker.id));
            }
            if marker.at.is_negative() {
                return Err(ModelError::NegativeMarker(marker.id));
            }
        }
        Ok(())
    }
}

/// 模型不变量被破坏的原因。文案由这里给出，界面只负责显示。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelError {
    /// 同一个 id 分配给了两个实体。
    DuplicateId(Id),
    /// 轨道上的片段重叠或没有按起点排序。
    Overlap(Id),
    /// 片段时长为 0 或素材出点早于入点。
    EmptyClip(Id),
    /// 转场引用的片段不存在，或者前后顺序反了。
    DanglingTransition(Id),
    NegativeMarker(Id),
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelError::DuplicateId(id) => write!(f, "id {id} 被重复分配"),
            ModelError::Overlap(track) => write!(f, "轨道 {track} 上的片段重叠或没有按起点排序"),
            ModelError::EmptyClip(clip) => write!(f, "片段 {clip} 的时长为 0，或素材出点早于入点"),
            ModelError::DanglingTransition(id) => {
                write!(f, "转场 {id} 引用的片段不存在，或前后顺序反了")
            }
            ModelError::NegativeMarker(id) => write!(f, "标记 {id} 在负时间上"),
        }
    }
}

impl std::error::Error for ModelError {}

#[cfg(test)]
mod tests;
