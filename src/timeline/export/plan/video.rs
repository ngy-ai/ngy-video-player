//! 视频这一侧的滤镜图：每段素材一条链，接效果 / 调色 / 关键帧 / 遮罩，段间 `xfade`。
//!
//! 遮罩在这里落地：形状先渲成一张 alpha 图（PGM 输入），`alphamerge` 上去，整条轨
//! 因此转成 yuva420p——叠的时候透明区才能露出下面的轨道（黑底在 `plan.rs` 里垫）。

use std::path::PathBuf;

use super::{ExportError, HDR_TO_SDR, Stream, input_index};
use crate::timeline::export::MaskAttachment;
use crate::timeline::{AssetId, Project, Sequence, Tick, Track};

/// 一道要拼进 concat 的画面：一条或几条滤镜链，加上最终输出标签。
struct Piece {
    /// 按顺序发出去的链，**每条链尾都带着自己的输出标签**。合并了转场时，
    /// 前面几段的链也在这里面；最后一条链（xfade）的标签就是 [`Piece::label`]。
    chains: Vec<String>,
    /// 最终输出标签，进 concat 的就是它。
    label: String,
    start: Tick,
    end: Tick,
    /// 空档为 `None`：转场只认片段，不认黑场。
    clip: Option<crate::timeline::Id>,
}

impl Piece {
    fn len(&self) -> Tick {
        self.end - self.start
    }
}

pub(super) fn video_stream(
    project: &Project,
    track: &Track,
    sequence: &Sequence,
    duration: Tick,
    inputs: &mut Vec<PathBuf>,
    masks: &mut Vec<MaskAttachment>,
) -> Result<Stream, ExportError> {
    let mut cursor = Tick::ZERO;
    let mut gap_index = 0;
    // `color` 的尺寸必须写成 `WxH`：滤镜参数用 `:` 分隔，`W:H` 会被当成两个参数。
    // `scale` / `pad` 反过来只能用 `W:H`（它们是位置参数 w/h/x/y）。
    let color_size = format!("{}x{}", sequence.width, sequence.height);
    let dims = format!("{}:{}", sequence.width, sequence.height);
    let fps = format!("{}/{}", sequence.fps.num(), sequence.fps.den());
    let mut pieces: Vec<Piece> = Vec::new();
    // 这条轨上有没有生效的遮罩。有的话整条轨都要带 alpha（concat 要求所有段同格式），
    // 否则 `alphamerge` 出来的 yuva420p 和黑场的 yuv420p 拼不到一起。
    let masked = track.clips.iter().any(|clip| clip.mask.effective());
    // 没遮罩的段也要补成 yuva420p：alpha 全 1，叠上去和以前一模一样。
    let alpha_suffix = if masked { ",format=yuva420p" } else { "" };

    for clip in &track.clips {
        // 空档补黑：不补的话 concat 会把后面的画面整体提前。
        if clip.start > cursor {
            // 带上轨道 id：多条轨都有空档时，各轨的 gap_index 都从 0 开始，
            // 不带轨道会产出两个同名 `[bgv0]`，ffmpeg 会拒绝重复的标签。
            let label = format!("[bgv{}_{gap_index}]", track.id.raw());
            pieces.push(Piece {
                chains: vec![format!(
                    "color=c=black:s={color_size}:r={fps}:d={:.6}{alpha_suffix}{label}",
                    (clip.start - cursor).secs()
                )],
                label,
                start: cursor,
                end: clip.start,
                clip: None,
            });
            gap_index += 1;
        }

        let index = input_index(inputs, project, clip.asset_id)?;

        // 交叉溶解要多取一段前一段的素材（往后延）和本片段之前的素材（往前挪）。
        // 前一段没有余量就退化成硬切——宁可切得生硬，也不要 ffmpeg 因缺帧把时长算错。
        let mut head = Tick::ZERO;
        let mut kind = None;
        // 变速片段不接转场：`xfade` 的时长按时间线算，而多取的素材按素材时间算，
        // 两头一换算就容易错帧。宁可退化成硬切，也不要成片时长对不上。速度曲线的
        // 情况更糟：多取的那一段在素材时间上的落点得反解曲线，索性一起排除。
        let rate = clip.rate();
        let steady = (rate - 1.0).abs() < 1e-6 && !clip.has_remap();
        if let Some(previous) = pieces.last()
            && previous.end == clip.start
            && steady
            && let Some(previous_id) = previous.clip
            && track
                .clip(previous_id)
                .is_some_and(|before| !before.has_remap())
            && let Some(transition) = track
                .transitions
                .iter()
                .find(|transition| transition.before == previous_id && transition.after == clip.id)
        {
            let usable = transition.duration.min(previous.len()).min(clip.source_in);
            if usable > Tick::ZERO {
                head = usable;
                kind = Some(transition.kind);
            }
        }

        // HDR 素材先转成 SDR 再缩放：成片的色彩空间固定是 bt709（见 ADR 0007），
        // 混进一段没做 tonemap 的 HLG 画面，整条片子都会发灰。
        let tonemap = if asset_is_hdr(project, clip.asset_id) {
            format!(",{HDR_TO_SDR}")
        } else {
            String::new()
        };
        // 视频必须用 `trim` + `setpts`：`atrim` 是音频滤镜，它的输出 pad 是 audio，
        // 接到 `setpts` 的 video 输入上会让整张 filter_complex 图连不起来。
        // 效果链排在缩放补边之后、转场之前：构图先定下来，降噪 / 锐化 / 水印
        // 都作用在成片尺寸上，字号与锐化强度才不会随素材分辨率乱跳。
        let effects = crate::timeline::effect_chain(sequence, clip.id);
        // 关键帧排在最前（缩放 / 位置 → 不透明度 → 效果）：先把画面构图推到该有的
        // 样子，再压噪点、再盖水印，水印不会跟着推近一起放大。
        let motion = match crate::timeline::keyframe_motion(&clip.keys, sequence) {
            filter if filter.is_empty() => String::new(),
            filter => format!(",{filter}"),
        };
        // 调色排在构图之后、淡入淡出之前：先定画面尺寸与位移，再统一色彩，
        // 最后才做不透明度。水印在 `effects` 里，排在后面，不会被调色一起染上。
        let color = match clip.color.filter_chain(project.base_dir()) {
            chain if chain.is_empty() => String::new(),
            chain => format!(",{chain}"),
        };
        let fades = crate::timeline::keyframe_fades(&clip.keys, clip.duration())
            .iter()
            .map(|filter| format!(",{filter}"))
            .collect::<String>();
        let label = format!("[v{}]", clip.id.raw());
        // 输出标签必须拼在链尾：concat 与 xfade 只认标签，链上没有标签时
        // ffmpeg 会报 "Stream specifier 'v3' ... matches no streams"。
        // 变速：`setpts` 按倍率压缩 / 拉伸时间戳，音轨那边用 `atempo` 对齐。
        // 速度曲线换成一条分段线性的表达式（`remap::setpts_filter`，单位已经换回时基）：
        // 常量倍率只是它的一个特例，但常量走短写法更清楚。
        let setpts = if clip.has_remap() {
            crate::timeline::setpts_filter(&clip.remap, clip.span().secs(), rate)
                .unwrap_or_else(|| "setpts=PTS-STARTPTS".to_string())
        } else if (rate - 1.0).abs() < 1e-6 {
            "setpts=PTS-STARTPTS".to_string()
        } else {
            format!("setpts=(PTS-STARTPTS)/{rate:.6}")
        };
        // 片尾定格：克隆最后一帧停 `hold` 秒，音轨补等长静音。
        let freeze = if clip.hold > Tick::ZERO {
            format!(
                ",tpad=stop_mode=clone:stop_duration={:.6}",
                clip.hold.secs()
            )
        } else {
            String::new()
        };
        let body = format!(
            "[{index}:v]trim=start={:.6}:end={:.6},{setpts}{freeze},\
             scale={dims}:force_original_aspect_ratio=decrease,\
             pad={dims}:(ow-iw)/2:(oh-ih)/2,setsar=1,fps={fps}{tonemap}{motion}{color}{fades}{effects}",
            (clip.source_in - head).secs(),
            clip.source_out.secs()
        );

        // 遮罩：形状渲成一张 alpha 图（PGM 输入，任务启动时才落盘），
        // 在效果与调色都算完之后 `alphamerge` 上去——遮罩切的是最终画面。
        let chain_list = if clip.mask.effective() {
            let mask_input = inputs.len();
            // 占位路径：任务层写好 PGM 之后按 `masks[i].input` 换掉。
            inputs.push(PathBuf::new());
            masks.push(MaskAttachment {
                input: mask_input,
                pgm: clip.mask.pgm(sequence.width, sequence.height),
            });
            let alpha = format!("[mk{}]", clip.id.raw());
            let pre = format!("[pre{}]", clip.id.raw());
            vec![
                format!("{body}{pre}"),
                // 遮罩是一张静止图：`loop` 出无限长的同帧，`trim` 到这一段的长度，
                // 让 `alphamerge` 有一个与画面等长、逐帧对齐的 alpha 输入。
                format!(
                    "[{mask_input}:v]format=gray,loop=loop=-1:size=1:start=0,\
                     fps={fps},trim=duration={:.6},setpts=PTS-STARTPTS{alpha}",
                    clip.duration().secs()
                ),
                format!("{pre}{alpha}alphamerge,format=yuva420p{label}"),
            ]
        } else {
            vec![format!("{body}{alpha_suffix}{label}")]
        };

        match kind {
            Some(kind) => {
                let previous = pieces.pop().expect("有转场就一定有前一段");
                // `xfade` 从 `offset` 开始溶到本片段：offset 落在前一段的尾巴上，
                // 也就是「前一段总长 - 溶接时长」。两段加起来仍等于原时长。
                // 两个 label 都自带方括号，直接相连：`[[v4]]` 会被 ffmpeg 当成
                // 一个不存在的滤镜名（报 "No such filter: ']xfade'"）。
                let xfade = format!(
                    "{}{label}xfade=transition={}:duration={:.6}:offset={:.6}[xf{}]",
                    previous.label,
                    xfade_name(kind),
                    head.secs(),
                    (previous.len() - head).secs().max(0.0),
                    clip.id.raw()
                );
                let mut chains = previous.chains;
                chains.extend(chain_list);
                chains.push(xfade);
                pieces.push(Piece {
                    chains,
                    label: format!("[xf{}]", clip.id.raw()),
                    start: previous.start,
                    end: clip.end(),
                    clip: Some(clip.id),
                });
            }
            None => pieces.push(Piece {
                chains: chain_list,
                label,
                start: clip.start,
                end: clip.end(),
                clip: Some(clip.id),
            }),
        }
        cursor = clip.end();
    }

    // 这条轨没铺满序列：补齐到整条序列的长度。多层 overlay 要求等长，
    // 而 `overlay` 默认按最短的一条截断——少这一段的成片会缺尾巴。
    if cursor < duration {
        let label = format!("[bgv{}_{gap_index}]", track.id.raw());
        pieces.push(Piece {
            chains: vec![format!(
                "color=c=black:s={color_size}:r={fps}:d={:.6}{alpha_suffix}{label}",
                (duration - cursor).secs()
            )],
            label,
            start: cursor,
            end: duration,
            clip: None,
        });
    }

    let mut stream = Stream::new();
    stream.masked = masked;
    for piece in pieces {
        // 黑场、片段、xfade 的链都自带输出标签，和 audio_stream 的 push 一样；
        // 这里只收集，不能再拼一次，否则转场那条链会多出一个标签。
        stream.chains.extend(piece.chains);
        stream.segments.push(piece.label);
    }
    Ok(stream)
}

/// 转场类型 → `xfade` 的 `transition` 取值。
fn xfade_name(kind: crate::timeline::TransitionKind) -> &'static str {
    match kind {
        crate::timeline::TransitionKind::CrossDissolve => "dissolve",
        crate::timeline::TransitionKind::Fade => "fade",
        crate::timeline::TransitionKind::Wipe => "wipeleft",
        crate::timeline::TransitionKind::Slide => "slideleft",
    }
}

/// 这段素材是不是 HDR。没探测过（老工程、音频文件）按 SDR 处理：
/// 多插一条 tonemap 链要多一道浮点转换，猜错比漏做更贵。
fn asset_is_hdr(project: &Project, asset_id: AssetId) -> bool {
    project
        .asset(asset_id)
        .and_then(|asset| asset.media.as_ref())
        .is_some_and(|media| media.color.is_hdr())
}
