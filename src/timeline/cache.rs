//! 预览渲染缓存（M10-T19）：把「效果栈 / 调色 / 关键帧」压过的片段预先渲染成小文件。
//!
//! 为什么需要它：预览走的是一条解码链（mpv 直接放素材），画面上的效果、调色、
//! 推近全都不经过它——那些东西只在导出时出现在 `filter_complex` 里。三条轨加降噪
//! 与推近的时间线，预览要么看不到效果，要么（真做上）实时算不动。做法是把每个
//! 「算不动」的片段单独渲成一个成片尺寸的缓存文件，预览改放这份文件：效果看得见，
//! 解码也不比原片重。
//!
//! 三条规则：
//!
//! * **指纹按「片段 + 效果参数」算**：素材路径、入出点、序列规格、效果 / 调色 /
//!   关键帧参数任何一项变了，指纹就变，旧缓存自然不会被命中。
//! * **只缓存能一对一映射时间的片段**：倍率必须是 1、片尾定格必须是 0。否则缓存文件
//!   的时间轴与片段在序列上的时间轴对不上，预览会越放越偏。
//! * **目录容量按 LRU 收缩**：缓存是「算得出来的东西」，丢了只是下次再算一遍。
//!
//! 这里只有纯函数（指纹、落点、清理计划），不碰 ffmpeg、不碰窗口。

use crate::timeline::export::{
    EncodeSpeed, ExportError, ExportPlan, ExportSettings, Quality, VideoCodec, plan_sequence_id,
};
use crate::timeline::{ClipItem, Id, Project, Sequence, Tick, Track, TrackKind};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// 缓存文件的后缀。与代理（`.proxy.mp4`）区分开：代理是原片的低码率副本，
/// 缓存是「加过效果的成片尺寸片段」，导出两者都不认。
pub const CACHE_SUFFIX: &str = "cache.mp4";

/// 缓存目录的容量上限：默认 4 GB，可调范围 0.5–64 GB。
pub const DEFAULT_LIMIT_MB: u64 = 4096;
pub const MIN_LIMIT_MB: u64 = 512;
pub const MAX_LIMIT_MB: u64 = 65_536;

/// 缓存目录在缓存根下面的名字。
pub const DIR_NAME: &str = "render-cache";

/// 这个片段值不值得预渲染。
///
/// 判据是「预览少它就会看出差别」：有启用的效果、调过色、或者打了会改画面的关键帧。
/// 同时要求倍率 1、没有片尾定格、没有速度曲线——只有这些条件成立时，缓存文件里的
/// 第 N 秒才正好是片段在序列上的第 N 秒。速度曲线本来也能对上（缓存走的是同一条
/// `setpts` 链），但预览那边对曲线片段是逐拍 seek，命中缓存也不会因此变顺，
/// 索性不缓存，省得指纹里再挂一条曲线参数。
pub fn worth_caching(clip: &ClipItem) -> bool {
    if (clip.rate() - 1.0).abs() > 1e-4
        || clip.hold > crate::timeline::Tick::ZERO
        || clip.has_remap()
    {
        return false;
    }
    let effects = clip
        .effects
        .iter()
        .any(|effect| effect.enabled && effect.kind != crate::timeline::EffectKind::Unknown);
    effects || !clip.color.is_neutral() || !clip.keys.is_empty()
}

/// 缓存指纹：只看「决定画面长什么样」的那些参数。
///
/// 用自己写的 FNV-1a 而不是 `DefaultHasher`：标准库不保证哈希值跨版本稳定，
/// 换个 Rust 版本就整目录缓存作废（或更糟：撞上旧文件的指纹）。
pub fn fingerprint(project: &Project, sequence: &Sequence, clip: &ClipItem) -> u64 {
    let mut text = String::new();
    let asset = project
        .asset_path(clip.asset_id)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let _ = write!(
        text,
        "{}|{}x{}|{:?}|{}|{}|{}|{}",
        asset,
        sequence.width,
        sequence.height,
        sequence.fps,
        clip.source_in.micros(),
        clip.source_out.micros(),
        serde_json::to_string(&clip.effects).unwrap_or_default(),
        serde_json::to_string(&clip.color).unwrap_or_default(),
    );
    let _ = write!(
        text,
        "|{}",
        serde_json::to_string(&clip.keys).unwrap_or_default()
    );
    fnv1a(text.as_bytes())
}

/// FNV-1a 64：一行就能实现的稳定哈希。
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

/// 缓存文件的落点：`<缓存目录>/<素材名>-<指纹>.cache.mp4`。
///
/// 名字里带素材名是为了让用户打开目录时认得出这是哪一段，指纹才是真正的键。
pub fn cache_path(root: &Path, project: &Project, sequence: &Sequence, clip: &ClipItem) -> PathBuf {
    let stem = project
        .asset(clip.asset_id)
        .map(|asset| asset.display_name())
        .map(|name| {
            name.rsplit_once('.')
                .map(|(stem, _)| stem.to_string())
                .unwrap_or(name)
        })
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "clip".to_string());
    let stem: String = stem
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    root.join(format!(
        "{stem}-{:016x}.{CACHE_SUFFIX}",
        fingerprint(project, sequence, clip)
    ))
}

/// 渲染缓存用的编码参数：比成片低一档，快、小，够预览看。
///
/// 预览不该为了「像素级一样」去跑慢档：缓存是给人看节奏与效果对不对的，
/// 真要看画质就导出一次。
pub fn cache_settings() -> ExportSettings {
    ExportSettings {
        codec: VideoCodec::H264,
        quality: Quality::Standard,
        speed: EncodeSpeed::Fast,
        faststart: true,
        ..ExportSettings::default()
    }
}

/// 给一个片段生成「只渲这一段」的导出计划。
///
/// 做法是造一条临时序列（一条视频轨、只有一个片段、从 0 开始）并把工程克隆一份，
/// 然后把克隆里的序列换成它——这样走的是导出那条一模一样的 `plan_sequence_id`：
/// 缩放补边、tonemap、关键帧、调色、效果、帧率全都不用在这里重写一遍。预览与
/// 导出「同一个滤镜链」这条约定靠的就是这个。
///
/// 前置条件由 [`worth_caching`] 保证：倍率 1、无定格，于是缓存文件的时间轴与
/// 片段在序列上的时间轴一一对应。
pub fn cache_plan(
    project: &Project,
    sequence: &Sequence,
    clip: &ClipItem,
) -> Result<ExportPlan, ExportError> {
    let mut temp = Sequence::new("cache", sequence.fps, sequence.width, sequence.height);
    temp.tracks.clear();
    let mut item = clip.clone();
    item.id = Id::new(1);
    item.start = Tick::ZERO;
    item.hold = Tick::ZERO;
    item.speed = 1.0;
    let mut track = Track::new(Id::new(2), TrackKind::Video, TrackKind::Video.label());
    track.clips.push(item);
    temp.tracks.push(track);
    let temp_id = temp.id;

    // 克隆工程只为借 `plan_sequence_id`（它按序列 id 去工程里取序列）。
    let mut clone = project.clone();
    clone.sequences = vec![temp];
    clone.active_sequence = temp_id;
    plan_sequence_id(&clone, temp_id, None, &cache_settings())
}

/// 目录里的一项。清理计划只需要这三个数。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheEntry {
    pub path: PathBuf,
    pub bytes: u64,
    /// 修改时间（Unix 秒）。读不出来时按 0：它的优先级最低，先被删。
    pub modified: u64,
}

/// 总占用（字节）。
pub fn total_bytes(entries: &[CacheEntry]) -> u64 {
    entries.iter().map(|entry| entry.bytes).sum()
}

/// 按 LRU 收缩到 `limit_bytes` 以内：返回要删的文件，最旧的在前。
///
/// 用修改时间而不是访问时间：Windows 上「最后访问时间」默认不更新，拿它当 LRU
/// 等于随机删。修改时间在这里近似「什么时候渲染出来的」，也够用——缓存内容由
/// 指纹决定，重算一次只是再花一遍时间。
pub fn trim_plan(entries: &[CacheEntry], limit_bytes: u64) -> Vec<PathBuf> {
    let mut total = total_bytes(entries);
    if total <= limit_bytes {
        return Vec::new();
    }
    let mut sorted = entries.to_vec();
    sorted.sort_by_key(|entry| (entry.modified, entry.path.clone()));
    let mut victims = Vec::new();
    for entry in sorted {
        if total <= limit_bytes {
            break;
        }
        total = total.saturating_sub(entry.bytes);
        victims.push(entry.path);
    }
    victims
}

/// 人类可读的容量文案（`512 MB` / `4.0 GB`）。
pub fn size_label(bytes: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let megabytes = bytes as f64 / MB;
    if megabytes >= 1024.0 {
        format!("{:.1} GB", megabytes / 1024.0)
    } else {
        format!("{megabytes:.0} MB")
    }
}

#[cfg(test)]
mod tests;
