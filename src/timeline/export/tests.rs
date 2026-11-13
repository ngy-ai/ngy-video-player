//! 导出计划的字符串形状：片段裁剪、空档补黑、输入去重、缺素材。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::*;
use crate::timeline::{AssetId, ClipItem, Id, Project, Tick, TrackKind};

// 区间导出单开一个文件，见 `tests/range.rs`；调色的链顺序见 `tests/color.rs`；
// 混音与轨道音量见 `tests/audio.rs`。
#[path = "tests/audio.rs"]
mod audio;
#[path = "tests/color.rs"]
mod color;
#[path = "tests/range.rs"]
mod range;
// 速度曲线（时间重映射）在计划里的形状见 `tests/remap.rs`。
#[path = "tests/remap.rs"]
mod remap;
#[path = "tests/sequences.rs"]
mod sequences;
#[path = "tests/subtitle.rs"]
mod subtitle;

fn s(n: i64) -> Tick {
    Tick::from_micros(n * 1_000_000)
}

/// 钉住 `-filter_complex` 的连通性：每条链都要以输出标签结尾，链里引用的
/// 内部标签都得有另一条链产出，且不能重复定义。
///
/// 少一个标签时 ffmpeg 只会说 "Stream specifier 'v3' ... matches no streams"，
/// 这类错误只有真调 ffmpeg 才看得见；放到纯字符串单测里，改滤镜链时就能当场发现。
fn assert_connected(filter: &str) {
    let mut produced = HashSet::new();
    let mut consumed = HashSet::new();

    for chain in filter.split(';').map(str::trim).filter(|c| !c.is_empty()) {
        assert!(chain.ends_with(']'), "链没有输出标签：{chain}");
        // 标签本身带方括号，拼链时不能再套一层：`[[v4]]xfade` 会让 ffmpeg
        // 把 `]xfade` 当滤镜名。合法的多输入链是 `[a][b]concat`，只有 `][`。
        assert!(!chain.contains("[["), "链里有重复的左括号：{chain}");
        assert!(!chain.contains("]]"), "链里有重复的右括号：{chain}");
        // 标签后面直接跟逗号 = 中间夹了一个空滤镜，ffmpeg 报的是
        // "No such filter: ''"，从名字上完全看不出是这里错了。
        assert!(
            !chain.contains("],"),
            "标签后面紧跟逗号，等于一个空滤镜：{chain}"
        );
        // `[0:v]` / `[0:a]` 是输入流说明符，不算内部标签。
        let labels: Vec<String> = chain
            .split('[')
            .skip(1)
            .filter_map(|tail| tail.split_once(']').map(|(name, _)| name))
            .filter(|name| !name.contains(':'))
            .map(str::to_string)
            .collect();
        // `split` / `asplit` 一条链会吐出好几个标签（速度曲线的音频分段就靠它），
        // 这些标签全是输出，不能套「最后一个才是输出」的规则。
        if chain.contains("asplit=") {
            for output in labels {
                assert!(
                    produced.insert(output.clone()),
                    "标签 {output} 被重复定义：{chain}"
                );
            }
            continue;
        }
        let (output, inputs) = labels.split_last().expect("至少有一个输出标签");
        assert!(
            produced.insert(output.clone()),
            "标签 {output} 被重复定义：{chain}"
        );
        consumed.extend(inputs.iter().cloned());
    }

    let missing: Vec<_> = consumed.difference(&produced).collect();
    assert!(
        missing.is_empty(),
        "滤镜图引用了没有出处的标签 {missing:?}：{filter}"
    );
}

/// 临时目录：`add_asset` 会判断文件在不在，所以测试得真的造一个文件。
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("ngy-export-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建临时目录");
        Self { dir }
    }

    /// 写一个 1 字节的假素材：登记只看文件在不在，不解码。
    fn media(&self, name: &str) -> PathBuf {
        let path = self.dir.join(name);
        std::fs::create_dir_all(path.parent().expect("有父目录")).expect("建素材目录");
        std::fs::write(&path, b"x").expect("写素材");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// 建一个只有一条视频轨的工程，片段都引用同一个素材。
fn project_with(scratch: &Scratch, clips: &[(Tick, Tick, Tick)]) -> (Project, Vec<Id>) {
    let mut project = Project::new();
    let asset = project.add_asset(&scratch.media("media/a.mp4"));
    let track = project.sequence_mut().tracks[0].id;
    let mut ids = Vec::new();

    for (source_in, source_out, start) in clips {
        let id = project.sequence_mut().alloc_id();
        let lane = project.sequence_mut().track_mut(track).expect("视频轨存在");
        lane.clips
            .push(ClipItem::new(id, asset, *source_in, *source_out, *start));
        lane.sort_clips();
        ids.push(id);
    }

    (project, ids)
}

/// 效果栈按加入顺序接在缩放补边之后：先降噪再锐化才有意义，反了只是把噪点放大。
#[test]
fn 片段效果按顺序进滤镜链() {
    let scratch = Scratch::new("effects");
    let (mut project, ids) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    let clip = ids[0];
    crate::timeline::add_effect(
        project.sequence_mut(),
        clip,
        crate::timeline::EffectKind::Denoise,
    );
    crate::timeline::add_effect(
        project.sequence_mut(),
        clip,
        crate::timeline::EffectKind::Sharpen,
    );

    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    let denoise = plan.filter.find("hqdn3d").expect("有降噪");
    let sharpen = plan.filter.find("unsharp").expect("有锐化");
    assert!(denoise < sharpen, "{}", plan.filter);
    assert!(
        plan.filter.contains("setsar=1,fps=30/1"),
        "效果排在缩放与帧率之后：{}",
        plan.filter
    );

    // 关掉的效果不发声，但顺序信息还在工程里。
    let effect = project.sequence_mut().clip_mut(clip).unwrap().effects[1].id;
    crate::timeline::toggle_effect(project.sequence_mut(), clip, effect);
    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    assert!(!plan.filter.contains("unsharp"), "{}", plan.filter);
    assert!(plan.filter.contains("hqdn3d"), "{}", plan.filter);
}

/// 关键帧的滤镜排在补边之后、效果之前：先定构图再压噪点，水印不会跟着推近放大。
#[test]
fn 关键帧接在补边之后效果之前() {
    let scratch = Scratch::new("keys");
    let (mut project, ids) = project_with(&scratch, &[(s(0), s(3), Tick::ZERO)]);
    let clip = ids[0];
    {
        let item = project.sequence_mut().clip_mut(clip).expect("片段存在");
        item.keys
            .ensure(crate::timeline::Property::Scale)
            .insert(Tick::ZERO, 1.0);
        item.keys
            .ensure(crate::timeline::Property::Scale)
            .insert(s(2), 2.0);
        item.keys
            .ensure(crate::timeline::Property::Opacity)
            .insert(Tick::ZERO, 0.0);
        item.keys
            .ensure(crate::timeline::Property::Opacity)
            .insert(s(1), 1.0);
    }
    crate::timeline::add_effect(
        project.sequence_mut(),
        clip,
        crate::timeline::EffectKind::Sharpen,
    );

    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    let pad = plan.filter.find("pad=").expect("有补边");
    let zoom = plan.filter.find("zoompan=").expect("有推近");
    let fade = plan.filter.find("fade=t=in").expect("有淡入");
    let sharpen = plan.filter.find("unsharp").expect("有锐化");
    assert!(pad < zoom, "{}", plan.filter);
    assert!(zoom < fade, "{}", plan.filter);
    assert!(fade < sharpen, "{}", plan.filter);
}

/// 变速：视频压时间戳、音频串 `atempo`；`atempo` 一档只认 0.5–2.0，超出的要串起来。
#[test]
fn 变速改时间戳并串联音频速率() {
    let scratch = Scratch::new("speed");
    let (mut project, ids) = project_with(&scratch, &[(s(0), s(4), Tick::ZERO)]);
    let clip = ids[0];
    project.sequence_mut().clip_mut(clip).unwrap().speed = 4.0;
    // 声音也要跟着变速率：同一条素材再往音频轨放一段（两条轨上的片段各设各的倍率）。
    let asset = project.assets[0].id;
    let audio = project.sequence_mut().tracks[1].id;
    let audio_id = project.sequence_mut().alloc_id();
    project
        .sequence_mut()
        .track_mut(audio)
        .expect("音频轨存在")
        .clips
        .push(ClipItem::new(audio_id, asset, s(0), s(4), Tick::ZERO));
    project.sequence_mut().clip_mut(audio_id).unwrap().speed = 4.0;

    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    assert!(
        plan.filter.contains("setpts=(PTS-STARTPTS)/4.000000"),
        "{}",
        plan.filter
    );
    assert!(
        plan.filter.contains("atempo=2.0,atempo=2.000000"),
        "4x 要串两档：{}",
        plan.filter
    );
}

/// 定格：视频端 `tpad` 克隆最后一帧，音频端补等长静音。
#[test]
fn 片尾定格克隆最后一帧并补静音() {
    let scratch = Scratch::new("hold");
    let (mut project, ids) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    let clip = ids[0];
    project.sequence_mut().clip_mut(clip).unwrap().hold = s(3);
    let asset = project.assets[0].id;
    let audio = project.sequence_mut().tracks[1].id;
    let audio_id = project.sequence_mut().alloc_id();
    project
        .sequence_mut()
        .track_mut(audio)
        .expect("音频轨存在")
        .clips
        .push(ClipItem::new(audio_id, asset, s(0), s(2), Tick::ZERO));
    project.sequence_mut().clip_mut(audio_id).unwrap().hold = s(3);

    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    assert!(
        plan.filter
            .contains("tpad=stop_mode=clone:stop_duration=3.000000"),
        "{}",
        plan.filter
    );
    assert!(
        plan.filter.contains("apad=pad_dur=3.000000"),
        "{}",
        plan.filter
    );
}

#[test]
fn 空时间线拒绝导出() {
    let project = Project::new();
    assert_eq!(plan_sequence(&project).unwrap_err(), ExportError::Empty);
}

#[test]
fn 视频轨全被隐藏时拒绝导出() {
    let scratch = Scratch::new("muted");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    project.sequence_mut().tracks[0].muted = true;
    assert_eq!(
        plan_sequence(&project).unwrap_err(),
        ExportError::NoVideoTrack
    );
}

#[test]
fn 单片段导出包含裁剪与拼接() {
    let scratch = Scratch::new("single");
    let (project, _) = project_with(&scratch, &[(s(1), s(3), Tick::ZERO)]);
    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);

    assert_eq!(plan.inputs.len(), 1);
    // 带输入 pad 一起钉：`atrim=` 也含 "trim="，只查后者会把音频滤镜放过去。
    assert!(
        plan.filter
            .contains("[0:v]trim=start=1.000000:end=3.000000,setpts=PTS-STARTPTS"),
        "{}",
        plan.filter
    );
    assert!(
        plan.filter.contains("concat=n=1:v=1:a=0[outv]"),
        "{}",
        plan.filter
    );
    assert_eq!(plan.video_label.as_deref(), Some("[outv]"));
    assert!(plan.audio_label.is_none(), "没有音频轨");
    assert_eq!(plan.inputs[0].file_name().unwrap(), "a.mp4");
}

#[test]
fn 同一素材的两个片段只算一个输入() {
    let scratch = Scratch::new("dedupe");
    let (project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO), (s(0), s(2), s(2))]);
    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);

    assert_eq!(plan.inputs.len(), 1, "同一个文件只 -i 一次");
    assert!(plan.filter.contains("concat=n=2"), "{}", plan.filter);
    assert!(
        plan.filter.matches("[0:v]").count() == 2,
        "两个片段都指向输入 0：{}",
        plan.filter
    );
}

#[test]
fn 片段之间的空档补黑场() {
    let scratch = Scratch::new("gap");
    let (project, _) = project_with(&scratch, &[(s(0), s(1), Tick::ZERO), (s(0), s(1), s(3))]);
    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);

    assert!(plan.filter.contains("color=c=black"), "{}", plan.filter);
    assert!(
        plan.filter.contains("d=2.000000"),
        "空档是 3 - 1 = 2 秒：{}",
        plan.filter
    );
    assert!(
        plan.filter.contains("concat=n=3"),
        "两段画面 + 一段黑场：{}",
        plan.filter
    );
}

#[test]
fn 音频轨参与导出并统一到四十八千赫() {
    let scratch = Scratch::new("audio");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    let asset = project.assets[0].id;
    let audio = project.sequence_mut().tracks[1].id;
    let id = project.sequence_mut().alloc_id();
    project
        .sequence_mut()
        .track_mut(audio)
        .expect("音频轨存在")
        .clips
        .push(ClipItem::new(id, asset, s(0), s(2), Tick::ZERO));

    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    assert!(plan.filter.contains("aresample=48000"), "{}", plan.filter);
    assert!(
        plan.filter.contains("concat=n=1:v=0:a=1[outa]"),
        "{}",
        plan.filter
    );
    assert_eq!(plan.audio_label.as_deref(), Some("[outa]"));
    assert!(
        plan.output_args.contains(&"aac".to_string()),
        "有音频就要编码音频"
    );
}

/// ffmpeg 的 filter_complex 是强类型的：`atrim` 输出音频 pad，接不上 `setpts`
/// 的视频输入；反过来的 `trim` → `asetpts` 一样连不起来。单看 "trim=" 分不出
/// 这两个词（`atrim` 里也有），所以按输入 pad 分别钉死。
#[test]
fn 视频与音频各用各的裁剪滤镜() {
    let scratch = Scratch::new("pads");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    let asset = project.assets[0].id;
    let audio = project.sequence_mut().tracks[1].id;
    let id = project.sequence_mut().alloc_id();
    project
        .sequence_mut()
        .track_mut(audio)
        .expect("音频轨存在")
        .clips
        .push(ClipItem::new(id, asset, s(0), s(2), Tick::ZERO));

    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    assert!(
        plan.filter
            .contains("[0:v]trim=start=0.000000:end=2.000000,setpts="),
        "{}",
        plan.filter
    );
    assert!(
        plan.filter
            .contains("[0:a]atrim=start=0.000000:end=2.000000,asetpts="),
        "{}",
        plan.filter
    );
    assert!(
        !plan.filter.contains("[0:v]atrim="),
        "视频链不能接音频滤镜：{}",
        plan.filter
    );
    assert!(
        !plan.filter.contains("[0:a]trim="),
        "音频链不能接视频滤镜：{}",
        plan.filter
    );
}

#[test]
fn 没有音频时显式关掉音频() {
    let scratch = Scratch::new("silent");
    let (project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    assert!(plan.output_args.contains(&"-an".to_string()));
    assert!(!plan.output_args.contains(&"aac".to_string()));
}

#[test]
fn 素材缺失在生成计划时就报错() {
    let scratch = Scratch::new("missing");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    project.assets[0].missing = true;

    match plan_sequence(&project).unwrap_err() {
        ExportError::MissingAsset(name) => assert_eq!(name, "a.mp4"),
        other => panic!("应当是缺素材，实际是 {other:?}"),
    }
}

#[test]
fn 代理素材拒绝导出() {
    // 工程里挂的是 `*.proxy.mp4`：预览拿它凑合没问题，导出会得到一份低码率成片，
    // 所以在生成计划这一步就拒绝，并让用户换回原片。
    let scratch = Scratch::new("proxy-source");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    let proxy = scratch.dir.join("a.proxy.mp4");
    std::fs::write(&proxy, b"x").unwrap();
    project.assets[0].path = PathBuf::from("a.proxy.mp4");
    project.assets[0].resolved = proxy;
    project.assets[0].missing = false;

    match plan_sequence(&project).unwrap_err() {
        ExportError::ProxySource(name) => assert_eq!(name, "a.proxy.mp4"),
        other => panic!("应当拒绝代理素材，实际是 {other:?}"),
    }
}

#[test]
fn 片段引用了不存在的素材时报错() {
    let mut project = Project::new();
    let track = project.sequence_mut().tracks[0].id;
    let id = project.sequence_mut().alloc_id();
    project
        .sequence_mut()
        .track_mut(track)
        .unwrap()
        .clips
        .push(ClipItem::new(id, AssetId::new(99), s(0), s(1), Tick::ZERO));

    assert_eq!(
        plan_sequence(&project).unwrap_err(),
        ExportError::UnknownAsset(99)
    );
}

#[test]
fn 参数拼装把输入滤镜与映射按顺序排好() {
    let scratch = Scratch::new("args");
    let (project, _) = project_with(&scratch, &[(s(0), s(1), Tick::ZERO)]);
    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    let args = plan.args();

    assert_eq!(args[0], "-i");
    assert!(args.contains(&"-filter_complex".to_string()));
    assert!(args.contains(&"[outv]".to_string()));
    assert!(args.contains(&"-crf".to_string()));
    // 输出文件由调用方追加：参数里只该有一个 .mp4（那个输入素材）。
    assert_eq!(
        args.iter().filter(|arg| arg.ends_with(".mp4")).count(),
        1,
        "输出文件不该出现在计划里"
    );
}

/// 往工程里挂一条新轨道并放一个片段。
fn add_track_clip(
    project: &mut Project,
    kind: TrackKind,
    asset: AssetId,
    source_in: Tick,
    source_out: Tick,
    start: Tick,
) -> Id {
    let track = project.sequence_mut().add_track(kind);
    let id = project.sequence_mut().alloc_id();
    project
        .sequence_mut()
        .track_mut(track)
        .expect("刚加的轨道")
        .clips
        .push(ClipItem::new(id, asset, source_in, source_out, start));
    id
}

#[test]
fn 两条视频轨自下而上叠加() {
    let scratch = Scratch::new("overlay");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    let asset = project.assets[0].id;
    // 新加的视频轨排在列表后面，也就是更下面的一层。
    add_track_clip(
        &mut project,
        TrackKind::Video,
        asset,
        s(0),
        s(2),
        Tick::ZERO,
    );

    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    assert!(plan.filter.contains("[layerv0]"), "{}", plan.filter);
    assert!(plan.filter.contains("[layerv1]"), "{}", plan.filter);
    assert!(
        plan.filter.contains("overlay=0:0:format=auto[outv]"),
        "最后一次 overlay 落到 [outv]：{}",
        plan.filter
    );
    assert_eq!(plan.video_label.as_deref(), Some("[outv]"));
    assert_eq!(plan.inputs.len(), 1, "同一个素材只 -i 一次");
}

#[test]
fn 视频轨比序列短时补黑对齐() {
    let scratch = Scratch::new("align");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(1), Tick::ZERO)]);
    let asset = project.assets[0].id;
    // 音频比视频长：序列总长 3 秒，视频轨只有 1 秒，得补 2 秒黑场。
    let audio = project.sequence_mut().tracks[1].id;
    let id = project.sequence_mut().alloc_id();
    project
        .sequence_mut()
        .track_mut(audio)
        .expect("音频轨存在")
        .clips
        .push(ClipItem::new(id, asset, s(0), s(3), Tick::ZERO));

    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    assert!(plan.filter.contains("d=2.000000"), "{}", plan.filter);
    assert!(
        plan.filter.contains("concat=n=2:v=1:a=0[outv]"),
        "原片段 + 补的黑场：{}",
        plan.filter
    );
}

#[test]
fn 相邻片段之间插交叉溶解() {
    let scratch = Scratch::new("xfade");
    let (mut project, ids) = project_with(&scratch, &[(s(1), s(3), Tick::ZERO)]);
    let asset = project.assets[0].id;
    // 紧贴前一段的第二个片段：source_in 要往前挪 0.5 秒做溶接。
    let track = project.sequence_mut().tracks[0].id;
    let second = project.sequence_mut().alloc_id();
    project
        .sequence_mut()
        .track_mut(track)
        .expect("视频轨存在")
        .clips
        .push(ClipItem::new(second, asset, s(2), s(4), s(2)));
    project
        .sequence_mut()
        .track_mut(track)
        .unwrap()
        .sort_clips();

    let transition = crate::timeline::Transition::new(
        project.sequence_mut().alloc_id(),
        ids[0],
        second,
        crate::timeline::TransitionKind::CrossDissolve,
        Tick::from_millis(500),
    );
    project
        .sequence_mut()
        .track_mut(track)
        .unwrap()
        .transitions
        .push(transition);

    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    assert!(
        plan.filter
            .contains("xfade=transition=dissolve:duration=0.500000"),
        "{}",
        plan.filter
    );
    assert!(
        plan.filter.contains("offset=1.500000"),
        "从第一段最后 0.5 秒开始溶：{}",
        plan.filter
    );
    assert!(
        plan.filter
            .contains("[0:v]trim=start=1.500000:end=4.000000"),
        "第二段往前多取 0.5 秒：{}",
        plan.filter
    );
    // 两段被 xfade 合成一段，concat 只看到一个。
    assert!(
        plan.filter.contains("concat=n=1:v=1:a=0[outv]"),
        "{}",
        plan.filter
    );
}

#[test]
fn 只有hdr素材才插tonemap链() {
    let scratch = Scratch::new("hdr");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(1), Tick::ZERO)]);

    let sdr = plan_sequence(&project).expect("能导出");
    assert!(
        !sdr.filter.contains("tonemap="),
        "SDR 素材不该多一道浮点转换：{}",
        sdr.filter
    );

    project.assets[0].media = Some(crate::timeline::MediaInfo {
        color: crate::timeline::ColorTags {
            range: "tv".to_string(),
            tags: "bt2020nc/bt2020/smpte2084".to_string(),
        },
        ..crate::timeline::MediaInfo::default()
    });
    let hdr = plan_sequence(&project).expect("能导出");
    assert!(hdr.filter.contains("tonemap=hable"), "{}", hdr.filter);
    assert!(hdr.filter.contains("zscale=t=bt709"), "{}", hdr.filter);
}

#[test]
fn 素材路径是绝对路径() {
    let scratch = Scratch::new("absolute");
    let (project, _) = project_with(&scratch, &[(s(0), s(1), Tick::ZERO)]);
    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    assert!(
        Path::new(&plan.inputs[0]).is_absolute(),
        "ffmpeg 子进程的工作目录不一定和工程一致"
    );
}

/// 每条轨的空档计数都从 0 开始：不带轨道 id 的话，两条轨会产出同名标签，
/// filter_complex 里同名输出标签是非法图。多轨 + 空档正好把这个撞车逼出来。
#[test]
fn 多条轨道都有空档时标签不撞车() {
    let scratch = Scratch::new("labels");
    let (mut project, _) = project_with(&scratch, &[(s(0), s(1), s(1))]);
    let asset = project.assets[0].id;

    // 序列总长 4 秒：两条视频轨都要补开头与末尾的黑场，两条音频轨都要补静音。
    let audio = project.sequence_mut().tracks[1].id;
    let id = project.sequence_mut().alloc_id();
    project
        .sequence_mut()
        .track_mut(audio)
        .expect("音频轨存在")
        .clips
        .push(ClipItem::new(id, asset, s(0), s(1), s(3)));
    add_track_clip(&mut project, TrackKind::Video, asset, s(0), s(1), s(2));
    add_track_clip(&mut project, TrackKind::Audio, asset, s(0), s(1), s(3));

    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    assert!(plan.filter.contains("overlay="), "{}", plan.filter);
    assert!(plan.filter.contains("amix="), "{}", plan.filter);
}
