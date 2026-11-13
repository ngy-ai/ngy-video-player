//! 分轨导出（stems）：每条音频轨单独出一个 wav。
//!
//! 这是 ADR 0011「不做 VST3/AU 宿主」的替代路径：需要降噪、混响这类插件时，把分轨
//! 导出到外部 DAW 里处理，再回导成素材放回时间线——宿主那一套（三套 ABI、进程外
//! 崩溃隔离、插件窗口托管）的成本远高于「导出去再导回来」。
//!
//! 每条轨沿用导出成片时的那一条音频链（`plan::audio_stream`）：推子、自动化、声像、
//! 速度曲线都已经在里面，所以分轨加起来和成片的那一轨听起来是同一个东西。
//! 空档仍然补静音，分轨的长度一律等于序列长度，导入 DAW 后可以直接对齐时间码。

use super::plan::{Stream, audio_stream};
use super::{ExportError, ExportPlan};
use crate::timeline::{Project, TrackKind};
use std::path::{Path, PathBuf};

/// 一条待导出的分轨。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StemPlan {
    /// 来自哪条音频轨（界面提示用）。
    pub track: u64,
    /// 轨道名，去掉文件名里不能用的字符。
    pub name: String,
    /// 落地文件。
    pub path: PathBuf,
    /// 喂给 `ExportTask` 的计划：只有音频流，输出 24bit wav。
    pub plan: ExportPlan,
}

/// 把序列里**每一条听得见的音频轨**展开成一条分轨计划。
///
/// 选轨规则与预览 / 成片一致（`Track::audible` + 独奏）：静音的、被独奏挤掉的、
/// 一条片段都没有的轨不导出——空的 wav 只会让用户在 DAW 里多关一个文件。
pub fn plan_stems(project: &Project, dir: &Path) -> Result<Vec<StemPlan>, ExportError> {
    let sequence = project.sequence();
    let duration = sequence.duration();
    if duration <= crate::timeline::Tick::ZERO {
        return Err(ExportError::Empty);
    }
    let solo = sequence.audio_solo();
    let mut stems = Vec::new();
    for track in sequence
        .tracks
        .iter()
        .filter(|track| track.kind == TrackKind::Audio)
    {
        if track.clips.is_empty() || !track.audible(solo) {
            continue;
        }
        let mut inputs = Vec::new();
        let stream = audio_stream(project, track, duration, &mut inputs)?;
        if stream.segments.is_empty() {
            continue;
        }
        let filter = concat_all(&stream);
        let index = stems.len() + 1;
        stems.push(StemPlan {
            track: track.id.raw(),
            name: sanitize(&track.name, index),
            path: dir.join(format!("{index:02}-{}.wav", sanitize(&track.name, index))),
            plan: ExportPlan {
                inputs,
                filter,
                video_label: None,
                audio_label: Some("[outa]".to_string()),
                // 24bit / 48kHz：DAW 里再加工时不至于先被 16bit 削掉一层动态。
                output_args: vec![
                    "-vn".to_string(),
                    "-c:a".to_string(),
                    "pcm_s24le".to_string(),
                    "-ar".to_string(),
                    "48000".to_string(),
                ],
                subtitle: None,
                masks: Vec::new(),
            },
        });
    }
    if stems.is_empty() {
        return Err(ExportError::NoAudioTrack);
    }
    Ok(stems)
}

/// 把这条轨已经按段拼好的链接成一条 `[outa]`。
fn concat_all(stream: &Stream) -> String {
    let mut filter = stream.chains.join(";");
    if !filter.is_empty() {
        filter.push(';');
    }
    filter.push_str(&format!(
        "{}concat=n={}:v=0:a=1[outa]",
        stream.segments.join(""),
        stream.segments.len()
    ));
    filter
}

/// 轨道名 → 文件名片段。
///
/// 只留安全字符：轨道名是用户随手输的，里面出现 `/` `:` 或者 emoji 都不奇怪，
/// 拼进路径要么创建失败、要么在 Windows 上落到别的目录去。
fn sanitize(name: &str, fallback: usize) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            ch if ch.is_control() => '_',
            ch => ch,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        format!("音轨{fallback}")
    } else {
        trimmed.chars().take(60).collect()
    }
}

#[cfg(test)]
mod tests;
