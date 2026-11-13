//! 音频这一侧的滤镜图：每段一条链，链尾落在 `[amN]`，段间 `concat`。
//!
//! 速度曲线在这里落地：按折点切成几块，各自恒定倍率 `atempo`，段长与积分严格相等。

use super::{AUDIO_RATE, ExportError, Stream, input_index};
use crate::timeline::{Project, Tick, Track};

use std::path::PathBuf;

/// 片段的声音主体：把素材区间 `[入点, 出点)` 变成一路音频，链尾统一落在 `[amN]` 上。
///
/// 返回「这几条链 + 末端的标签」：调用方在标签后面接着接采样率 / 格式 / 音量。
///
/// 常量倍率是一条 `atempo`（快慢各一档，极端倍率串联）。速度曲线不行：`atempo` 只认
/// 恒定倍率，只能按曲线折点切成几块、各自变速再 `concat` 拼回来；每块的倍率取
/// 「调和中项」，让它的时长与曲线积分严格相等（见 `timeline::remap::audio_chunks`）。
/// 同一条输入流被切进多条链之前要先 `asplit`：filter_complex 里一个输入标签只能用一次。
fn audio_body(clip: &crate::timeline::ClipItem, index: usize) -> (Vec<String>, String) {
    let id = clip.id.raw();
    let start = clip.source_in.secs();
    let raw = format!("[am{id}]");
    let chunks = crate::timeline::audio_chunks(&clip.remap, clip.span().secs(), clip.rate());
    // 常量倍率（含「曲线只有一个点」）：一条 `atrim` 就够。
    if !clip.has_remap() || chunks.len() <= 1 {
        let rate = chunks
            .first()
            .map_or_else(|| clip.rate(), |chunk| chunk.rate as f32);
        let tempo = atempo_chain(rate);
        return (
            vec![format!(
                "[{index}:a]atrim=start={start:.6}:end={:.6},asetpts=PTS-STARTPTS{tempo}{raw}",
                clip.source_out.secs()
            )],
            raw,
        );
    }

    // `asplit` 的输出标签要唯一：同一份工程里两个片段各切一次也不会撞名。
    let splits: Vec<String> = (0..chunks.len()).map(|i| format!("[ac{id}_{i}]")).collect();
    let mut chains = vec![format!(
        "[{index}:a]asplit={}{}",
        chunks.len(),
        splits.concat()
    )];
    for (i, (split, chunk)) in splits.iter().zip(&chunks).enumerate() {
        let tempo = atempo_chain(chunk.rate as f32);
        chains.push(format!(
            "{split}atrim=start={:.6}:end={:.6},asetpts=PTS-STARTPTS{tempo}[ad{id}_{i}]",
            start + chunk.start,
            start + chunk.end,
        ));
    }
    let done: String = (0..chunks.len()).map(|i| format!("[ad{id}_{i}]")).collect();
    chains.push(format!("{done}concat=n={}:v=0:a=1{raw}", chunks.len()));
    (chains, raw)
}

/// 倍率 → `atempo` 链。
///
/// `atempo` 一档只认 0.5–2.0，更极端的倍率要串联：4x 是 `atempo=2.0,atempo=2.0`，
/// 0.25x 是 `atempo=0.5,atempo=0.5`。串起来乘起来必须正好等于目标倍率。
fn atempo_chain(rate: f32) -> String {
    if (rate - 1.0).abs() < 1e-6 || !rate.is_finite() || rate <= 0.0 {
        return String::new();
    }
    let mut rest = f64::from(rate);
    let mut parts: Vec<String> = Vec::new();
    while rest > 2.0 {
        parts.push("atempo=2.0".to_string());
        rest /= 2.0;
    }
    while rest < 0.5 {
        parts.push("atempo=0.5".to_string());
        rest /= 0.5;
    }
    parts.push(format!("atempo={rest:.6}"));
    parts.iter().map(|part| format!(",{part}")).collect()
}

pub(crate) fn audio_stream(
    project: &Project,
    track: &Track,
    duration: Tick,
    inputs: &mut Vec<PathBuf>,
) -> Result<Stream, ExportError> {
    let mut stream = Stream::new();
    let mut cursor = Tick::ZERO;
    let mut gap_index = 0;
    // 轨道音量落成 `volume=`：它是振幅倍数，与预览侧乘到 mpv 音量上的那个数同量纲。
    // 静音段不用乘——`anullsrc` 多少倍都是静音，少插一道滤镜更清楚。
    let gain = track.normalized_gain();
    let volume = if (gain - 1.0).abs() < 1e-4 {
        String::new()
    } else {
        format!(",volume={gain:.4}")
    };

    for clip in &track.clips {
        if clip.start > cursor {
            let duration = (clip.start - cursor).secs();
            stream.push(
                format!(
                    "anullsrc=r={AUDIO_RATE}:cl=stereo,\
                     atrim=duration={duration:.6}"
                ),
                // 同理：静音段标签也要带轨道 id，避免多轨同名。
                format!("[bga{}_{gap_index}]", track.id.raw()),
            );
            gap_index += 1;
        }

        let index = input_index(inputs, project, clip.asset_id)?;
        let label = format!("[a{}]", clip.id.raw());
        // 声音主体：常量倍率是一条 `atempo`，速度曲线要分段变速（见 `audio_body`）。
        let (mut chains, raw) = audio_body(clip, index);
        // 定格段补静音，好和视频那边的 `tpad` 对齐。
        let pad = if clip.hold > Tick::ZERO {
            format!(",apad=pad_dur={:.6}", clip.hold.secs())
        } else {
            String::new()
        };
        // 三层音量（片段 → 自动化 → 推子）连乘，顺序见 `timeline::mixer`；
        // 连乘与顺序无关，分开写只是为了每一层都能单独看出它在哪儿。
        let clip_gain = crate::timeline::mixer::clip_gain(clip.gain);
        let clip_volume = if (clip_gain - 1.0).abs() < 1e-4 {
            String::new()
        } else {
            format!(",volume={clip_gain:.4}")
        };
        // 自动化用 `eval=frame`：每帧按轨道时间求值。滤镜链里这一段从 PTS 0 开始，
        // 所以表达式里把片段起点加回去。
        let automation = crate::timeline::mixer::gain_expression(track, clip.start)
            .map(|expr| format!(",volume=volume='{expr}':eval=frame"))
            .unwrap_or_default();
        // 声像走 `pan`：左右各乘一个系数（等功率），居中时不插这道。
        let (left, right) = crate::timeline::mixer::pan_gains(track.pan);
        let pan = if (left - right).abs() < 1e-4 {
            String::new()
        } else {
            format!(",pan=stereo|c0={left:.4}*c0|c1={right:.4}*c1")
        };
        chains.push(format!(
            "{raw}aresample={AUDIO_RATE},\
             aformat=sample_fmts=fltp:channel_layouts=stereo{pad}\
             {clip_volume}{automation}{volume}{pan}{label}"
        ));
        stream.push_chains(chains, label);
        cursor = clip.end();
    }

    // 跟视频一样补齐到整条序列：`amix` 的 duration=longest 才不会把别的轨
    // 提前砍掉，各条轨也都在同一个时间零点上。
    if cursor < duration {
        stream.push(
            format!(
                "anullsrc=r={AUDIO_RATE}:cl=stereo,atrim=duration={:.6}",
                (duration - cursor).secs()
            ),
            format!("[bga{}_{gap_index}]", track.id.raw()),
        );
    }

    Ok(stream)
}
