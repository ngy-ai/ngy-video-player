//! 字幕图层：字幕轨上按时间排布的文字条目，以及交给 libass 的 ASS 文本。
//!
//! 字幕不是素材片段：它没有素材入出点、没有倍率与效果，也不参与视频 / 音频选轨，
//! 所以单独用 [`SubtitleItem`] 挂在 `Track.subtitles` 上，而不是伪造一段 `ClipItem`
//! （后者会被 `Sequence::validate` 的素材区间校验和拖动 / 切割语义绊住）。
//!
//! 时间和整条时间线一样是 [`Tick`]（微秒整数）：ASS 的厘秒时间戳只在生成文本时换算，
//! 界面上的起止点、区间导出的裁切都用整数运算，不会出现「字幕比画面早一帧」。

mod ass;
mod ass_import;
mod text;

use serde::{Deserialize, Serialize};

use super::{Id, Sequence, Tick, TrackKind};

pub use ass::ass_script;
pub use ass_import::{StyledCue, parse as parse_ass, read as read_ass};
pub use text::{Cue as TextCue, Format, format_srt, format_vtt, parse as parse_text, read, write};

#[cfg(test)]
mod tests;

/// 样式里的字号档位按这个高度定义（1080p 下的像素）。脚本的 `PlayResY` 取序列高度，
/// 所以导出按高度放大、导入按高度折回——两边都落在同一套数字上，4K 工程不会换算一次就变一半。
pub const STYLE_HEIGHT: u32 = 1080;

/// 字号档位：按序列高度当 ASS 的 `PlayResY`，所以这里的数字就是 1080p 下的像素。
pub const MIN_SIZE: u32 = 12;
pub const MAX_SIZE: u32 = 96;
pub const SIZE_STEP: u32 = 4;
/// 描边宽度上限。再宽就把字糊成一团了。
pub const MAX_OUTLINE: u32 = 8;

/// 面板上可选的字体。都是系统常见的 sans 字体，中文由 libass 自行回退。
pub const FONTS: [&str; 3] = ["Arial", "Microsoft YaHei", "Noto Sans CJK SC"];
/// 面板上可选的文字颜色（`0xRRGGBB`）。ASS 里要反过来写成 BGR。
pub const COLORS: [(u32, &str); 5] = [
    (0xffffff, "白"),
    (0xffd54a, "黄"),
    (0x66e0ff, "青"),
    (0xff6b6b, "红"),
    (0x101014, "黑"),
];

/// 字幕在画面里的垂直位置，映射到 ASS 的 `Alignment`（小键盘布局）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtitlePosition {
    Top,
    Middle,
    #[default]
    Bottom,
}

pub const POSITIONS: [SubtitlePosition; 3] = [
    SubtitlePosition::Top,
    SubtitlePosition::Middle,
    SubtitlePosition::Bottom,
];

impl SubtitlePosition {
    pub fn label(self) -> &'static str {
        match self {
            Self::Top => "上",
            Self::Middle => "中",
            Self::Bottom => "下",
        }
    }

    /// ASS 的 alignment：1/2/3 是下排左中右，4/5/6 中排，7/8/9 上排。
    fn alignment(self) -> u32 {
        match self {
            Self::Bottom => 2,
            Self::Middle => 5,
            Self::Top => 8,
        }
    }

    /// 预览叠层用：底 / 中 / 顶对应 flex 的三个对齐位置。
    pub fn vertical(self) -> &'static str {
        match self {
            Self::Bottom => "end",
            Self::Middle => "center",
            Self::Top => "start",
        }
    }
}

/// 一条字幕的排版。字段都是值类型，随工程 JSON 存盘；未知枚举值由 serde 报错，
/// 所以位置写成字符串而不是索引，手改工程时不会「数字 3 不知道是什么」。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubtitleStyle {
    pub font: String,
    pub size: u32,
    /// `0xRRGGBB`。
    pub color: u32,
    /// 描边宽度（像素），0 表示不要描边。
    pub outline: u32,
    pub position: SubtitlePosition,
}

impl Default for SubtitleStyle {
    fn default() -> Self {
        Self {
            font: FONTS[0].to_string(),
            size: 48,
            color: COLORS[0].0,
            outline: 2,
            position: SubtitlePosition::Bottom,
        }
    }
}

impl SubtitleStyle {
    /// 手改过的工程、面板步进之后都在这里收口：不让坏值进 ASS 文本。
    pub fn normalize(&mut self) {
        if self.font.trim().is_empty() || !FONTS.contains(&self.font.as_str()) {
            self.font = FONTS[0].to_string();
        }
        self.size = self.size.clamp(MIN_SIZE, MAX_SIZE);
        self.outline = self.outline.min(MAX_OUTLINE);
        self.color &= 0x00ff_ffff;
    }

    pub fn summary(&self) -> String {
        let color = COLORS
            .iter()
            .find(|(rgb, _)| *rgb == self.color)
            .map(|(_, label)| *label)
            .unwrap_or("自定义");
        format!(
            "{} {}px · {} · 描边{} · {}",
            self.font,
            self.size,
            color,
            self.outline,
            self.position.label()
        )
    }
}

/// 字幕轨上的一条文字。`[start, end)` 是序列时间，与片段的起点同一套坐标。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubtitleItem {
    pub id: Id,
    pub start: Tick,
    pub end: Tick,
    pub text: String,
    pub style: SubtitleStyle,
}

impl Default for SubtitleItem {
    /// 缺字段的老工程读出来也有一条能编辑的字幕，而不是一个时长为 0 的坏条目。
    fn default() -> Self {
        Self {
            id: Id::default(),
            start: Tick::ZERO,
            end: Tick::from_secs(3.0),
            text: "新字幕".to_string(),
            style: SubtitleStyle::default(),
        }
    }
}

impl SubtitleItem {
    pub fn new(id: Id, start: Tick, end: Tick, text: impl Into<String>) -> Self {
        Self {
            id,
            start,
            end,
            text: text.into(),
            style: SubtitleStyle::default(),
        }
    }

    /// 时长为 0 的条目不会出现在导出与预览里。
    pub fn duration(&self) -> Tick {
        self.end.saturating_sub(self.start).max(Tick::ZERO)
    }

    pub fn covers(&self, at: Tick) -> bool {
        self.start <= at && at < self.end
    }

    /// 面板确定之前收口：时间、样式都不能带坏值进工程。
    pub fn normalize(&mut self) {
        self.start = self.start.max(Tick::ZERO);
        if self.end <= self.start {
            self.end = self.start.saturating_add(Tick::from_secs(3.0));
        }
        self.text = self.text.replace('\r', "").trim().to_string();
        if self.text.is_empty() {
            self.text = "新字幕".to_string();
        }
        self.style.normalize();
    }

    /// 时间线上显示的一行：换行压成空格，条目块很矮。
    pub fn label(&self) -> String {
        self.text.replace(['\n', '\\'], " ").trim().to_string()
    }
}

/// 参与输出的字幕条目：隐藏的字幕轨整条跳过，轨道顺序不变（靠上的先）。
pub fn items(sequence: &Sequence) -> impl Iterator<Item = &SubtitleItem> {
    sequence
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Subtitle && !track.muted)
        .flat_map(|track| track.subtitles.iter())
}

/// 播放头在 `at` 时该显示哪条字幕。轨道靠上优先，同一轨按起点有序取第一条。
pub fn at(sequence: &Sequence, at: Tick) -> Option<&SubtitleItem> {
    sequence
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Subtitle && !track.muted)
        .find_map(|track| track.subtitles.iter().find(|item| item.covers(at)))
}
