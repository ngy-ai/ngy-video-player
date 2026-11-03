use super::*;
use crate::clip::ClipRange;
use std::path::PathBuf;

fn request(options: ExportOptions) -> ClipRequest {
    ClipRequest {
        input: PathBuf::from("movie.mkv"),
        output: PathBuf::from(format!("movie - 片段.{}", options.format.extension())),
        range: ClipRange {
            start: 10.0,
            end: 25.0,
        },
        options,
    }
}

fn args(options: ExportOptions) -> Vec<String> {
    let encoder = options.codec.software();
    build_command(Path::new("ffmpeg"), &request(options), encoder)
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

fn has(args: &[String], needle: &[&str]) -> bool {
    args.windows(needle.len()).any(|slice| {
        slice
            .iter()
            .zip(needle)
            .all(|(arg, want)| arg.as_str() == *want)
    })
}

fn count(args: &[String], value: &str) -> usize {
    args.iter().filter(|arg| arg.as_str() == value).count()
}

#[test]
fn 默认仍然精确重编码为_h264() {
    let args = args(ExportOptions::default());
    assert!(has(&args, &["-c:v", "libx264"]));
    assert!(has(&args, &["-crf", "20"]));
    assert!(has(&args, &["-c:a", "aac"]));
    assert!(has(&args, &["-c:s", "copy"]));
    assert!(!args.iter().any(|arg| arg == "-vf"));
    assert!(!args.iter().any(|arg| arg == "-af"));
    assert!(has(&args, &["-map", "0:a?"]));
    assert!(has(&args, &["-map", "0:s?"]));
    assert!(has(&args, &["-map_chapters", "0"]));
    // 默认跟随源：不写 -pix_fmt，10bit 源才不会被压成 8bit。
    assert!(!args.iter().any(|arg| arg == "-pix_fmt"));
}

#[test]
fn 强制_8bit_时才写_pix_fmt() {
    let options = ExportOptions {
        pixel_format: PixelFormat::Yuv420p,
        ..ExportOptions::default()
    };
    assert!(has(&args(options), &["-pix_fmt", "yuv420p"]));
}

#[test]
fn 硬件编码器始终写死_8bit() {
    // NVENC / QSV / AMF 多数型号只吃 8bit 4:2:0，不指定 -pix_fmt 会初始化失败。
    let args: Vec<String> = build_command(
        Path::new("ffmpeg"),
        &request(ExportOptions::default()),
        "h264_nvenc",
    )
    .get_args()
    .map(|arg| arg.to_string_lossy().into_owned())
    .collect();
    assert!(has(&args, &["-pix_fmt", "yuv420p"]));
}

#[test]
fn 音频复制只给_codec_copy() {
    let options = ExportOptions {
        audio_copy: true,
        ..ExportOptions::default()
    };
    let args = args(options);
    assert!(has(&args, &["-c:a", "copy"]));
    assert!(!args.iter().any(|arg| arg == "-b:a"));
}

#[test]
fn 音频复制时音频滤镜与采样率一起消失() {
    let options = ExportOptions {
        audio_copy: true,
        normalize_audio: true,
        audio_offset: 0.3,
        sync_audio: true,
        ..ExportOptions::default()
    };
    // 勾了响度/偏移也不能落地：样本根本不解出来。
    assert!(!options.audio_filters_available());
    assert_eq!(options.audio_filters(), None);
    let args = args(options);
    assert!(!args.iter().any(|arg| arg == "-af"));
    assert!(!args.iter().any(|arg| arg == "-ar"));
}

#[test]
fn hls_下音频复制退化成_aac() {
    let options = ExportOptions {
        format: OutputFormat::Hls,
        audio_copy: true,
        normalize_audio: true,
        ..ExportOptions::default()
    };
    // TS 分片装不下 FLAC / Opus，复制会让整批导出失败。
    let args = args(options.clone());
    assert!(has(&args, &["-c:a", "aac"]));
    assert!(!has(&args, &["-c:a", "copy"]));
    // 既然退回了转码，音频滤镜就应该照常生效。
    assert!(options.audio_filters_available());
    assert!(args.iter().any(|arg| arg == "-af"));
}

#[test]
fn 无损流复制不带任何编码与滤镜参数() {
    let mut options = ExportOptions {
        mode: ExportMode::Copy,
        ..ExportOptions::default()
    };
    // 即使界面状态里滤镜还开着，流复制也必须忽略它们。
    options.tonemap_hdr = true;
    options.deinterlace = true;
    options.normalize_audio = true;
    let args = args(options);
    assert!(has(&args, &["-c", "copy"]));
    for forbidden in ["-c:v", "-crf", "-vf", "-af", "-c:a", "-c:s", "-pix_fmt"] {
        assert!(
            !args.iter().any(|arg| arg == forbidden),
            "流复制不该带 {forbidden}"
        );
    }
    assert!(has(&args, &["-map", "0:a?"]));
}

#[test]
fn 画质档位换算成_crf() {
    let crf = |quality| {
        let options = ExportOptions {
            quality,
            ..ExportOptions::default()
        };
        args(options)
            .windows(2)
            .find(|pair| pair[0] == "-crf")
            .map(|pair| pair[1].clone())
            .unwrap()
    };
    assert_eq!(crf(Quality::High), "18");
    assert_eq!(crf(Quality::Standard), "20");
    assert_eq!(crf(Quality::Small), "23");
}

#[test]
fn 滤镜按固定顺序串成一条链() {
    let options = ExportOptions {
        deinterlace: true,
        tonemap_hdr: true,
        denoise: true,
        sharpen: true,
        ..ExportOptions::default()
    };
    let filters = options.video_filters().unwrap();
    let deinterlace = filters.find("yadif").unwrap();
    let tonemap = filters.find("tonemap=tonemap=hable").unwrap();
    let denoise = filters.find("hqdn3d").unwrap();
    let sharpen = filters.find("unsharp").unwrap();
    assert!(deinterlace < tonemap && tonemap < denoise && denoise < sharpen);
    // 色调映射必须落回 yuv420p，否则后面的降噪/锐化会拿到浮点线性帧。
    assert!(filters.contains("tonemap=tonemap=hable:desat=0"));
    assert!(filters.contains("zscale=t=bt709:m=bt709:r=tv,format=yuv420p"));
}

#[test]
fn 响度标准化会补上采样率() {
    let options = ExportOptions {
        normalize_audio: true,
        ..ExportOptions::default()
    };
    let args = args(options);
    assert!(has(&args, &["-af", "loudnorm=I=-16:TP=-1.5:LRA=11"]));
    assert!(has(&args, &["-ar", "48000"]));
}

#[test]
fn 音频偏移正数延迟负数裁剪开头() {
    let ahead = ExportOptions {
        audio_offset: 0.3,
        ..ExportOptions::default()
    };
    assert_eq!(
        ahead.audio_filters().as_deref(),
        Some("adelay=300:all=1"),
        "声音延后 300ms"
    );

    let behind = ExportOptions {
        audio_offset: -0.25,
        ..ExportOptions::default()
    };
    assert_eq!(
        behind.audio_filters().as_deref(),
        Some("atrim=start=0.250,asetpts=PTS-STARTPTS"),
        "声音提前 250ms"
    );

    let both = ExportOptions {
        audio_offset: 0.2,
        normalize_audio: true,
        ..ExportOptions::default()
    };
    assert_eq!(
        both.audio_filters().as_deref(),
        Some("adelay=200:all=1,loudnorm=I=-16:TP=-1.5:LRA=11")
    );
}

#[test]
fn 漂移校正排在音频滤镜最前() {
    let options = ExportOptions {
        sync_audio: true,
        audio_offset: 0.2,
        normalize_audio: true,
        ..ExportOptions::default()
    };
    assert_eq!(
        options.audio_filters().as_deref(),
        Some(
            "aresample=async=1:first_pts=0,adelay=200:all=1,\
             loudnorm=I=-16:TP=-1.5:LRA=11"
        )
    );
}

#[test]
fn 时间戳修复在流复制下也生效() {
    let mut options = ExportOptions {
        fix_timestamps: true,
        ..ExportOptions::default()
    };
    let exact = args(options.clone());
    let fflags = exact.iter().position(|arg| arg == "-fflags").unwrap();
    let input = exact.iter().position(|arg| arg == "-i").unwrap();
    assert!(fflags < input, "genpts 必须在 -i 之前");
    assert_eq!(exact[fflags + 1], "+genpts");

    options.mode = ExportMode::Copy;
    let copy = args(options);
    assert!(has(&copy, &["-fflags", "+genpts"]));
    // 流复制仍然不能碰音频滤镜。
    assert!(!copy.iter().any(|arg| arg == "-af"));
}

#[test]
fn hls_输出播放列表与分片() {
    let options = ExportOptions {
        format: OutputFormat::Hls,
        ..ExportOptions::default()
    };
    let args = args(options);
    assert!(has(&args, &["-f", "hls"]));
    assert!(has(&args, &["-hls_time", "6"]));
    assert!(has(&args, &["-hls_playlist_type", "vod"]));
    assert!(has(
        &args,
        &["-hls_segment_filename", "movie - 片段_%04d.ts"]
    ));
    // TS 装不下章节/字幕，元数据也不带。
    assert!(has(&args, &["-map_metadata", "-1"]));
    assert!(has(&args, &["-map_chapters", "-1"]));
    assert!(!has(&args, &["-map", "0:s?"]));
    assert!(args.last().unwrap().ends_with(".m3u8"));
    // TS 分片里的 10bit H.264 支持面太窄，这条路固定 8bit。
    assert!(has(&args, &["-pix_fmt", "yuv420p"]));
}

#[test]
fn 恒定帧率只在重编码时出现() {
    let mut options = ExportOptions {
        force_cfr: true,
        ..ExportOptions::default()
    };
    let exact = args(options.clone());
    assert!(has(&exact, &["-fps_mode", "cfr"]));

    // 流复制改不了时间戳节奏，这个开关必须被忽略。
    options.mode = ExportMode::Copy;
    let copy = args(options);
    assert!(!copy.iter().any(|arg| arg == "-fps_mode"));
}

#[test]
fn 去掉音轨后不再有音频映射与编码() {
    let options = ExportOptions {
        keep_audio: false,
        ..ExportOptions::default()
    };
    let args = args(options);
    assert!(!has(&args, &["-map", "0:a?"]));
    assert!(!args.iter().any(|arg| arg == "-c:a"));
    assert!(!args.iter().any(|arg| arg == "-af"));
    assert_eq!(count(&args, "-map"), 2, "只留视频与字幕两路映射");
}

#[test]
fn 去掉字幕与章节时对应参数一起消失() {
    let options = ExportOptions {
        keep_subtitles: false,
        keep_chapters: false,
        ..ExportOptions::default()
    };
    let args = args(options);
    assert!(!has(&args, &["-map", "0:s?"]));
    assert!(!args.iter().any(|arg| arg == "-c:s"));
    assert!(has(&args, &["-map_chapters", "-1"]));
}

#[test]
fn 选_h265_时换编码器() {
    let options = ExportOptions {
        codec: VideoCodec::Hevc,
        ..ExportOptions::default()
    };
    let args = args(options);
    assert!(has(&args, &["-c:v", "libx265"]));
}

#[test]
fn 剪切区间落在输入之后与输出之前() {
    let args = args(ExportOptions::default());
    let input = args.iter().position(|arg| arg == "-i").unwrap();
    let start = args.iter().position(|arg| arg == "-ss").unwrap();
    let end = args.iter().position(|arg| arg == "-t").unwrap();
    assert!(start < input, "-ss 必须在 -i 前才能快点开始");
    assert!(input < end, "-t 是输出时长，跟在输入之后");
    assert_eq!(args[input + 1], "movie.mkv");
    assert_eq!(args[start + 1], "10.000");
    assert_eq!(args[end + 1], "15.000");
}

#[test]
fn gif_导出忽略流复制并走调色板() {
    let options = ExportOptions {
        format: OutputFormat::Gif,
        // 界面在 GIF 下不提供「导出方式」，但旧状态里可能还留着 Copy。
        mode: ExportMode::Copy,
        ..ExportOptions::default()
    };
    let args = args(options.clone());
    assert!(has(&args, &["-c:v", "gif"]));
    assert!(!has(&args, &["-c", "copy"]), "GIF 没有流复制这条路");
    let filters = args
        .iter()
        .position(|arg| arg == "-vf")
        .map(|index| args[index + 1].clone())
        .expect("GIF 必须有滤镜链");
    assert!(
        filters.ends_with("split[s0][s1];[s0]palettegen[p];[s1][p]paletteuse"),
        "调色板要在最后一段：{filters}"
    );
    assert!(has(&args, &["-loop", "0"]), "无限循环");
    // 动图没有音轨与字幕。
    assert!(!has(&args, &["-map", "0:a?"]));
    assert!(!args.iter().any(|arg| arg == "-c:a"));
    assert!(!args.iter().any(|arg| arg == "-c:s"));
    assert!(args.last().unwrap().ends_with(".gif"));
    assert!(options.filters_available(), "GIF 强制重编码，滤镜要可用");
}

#[test]
fn 画面变换排进滤镜链且在降噪之前() {
    let options = ExportOptions {
        denoise: true,
        transform: VideoTransform {
            crop: Crop {
                left: 10.0,
                right: 10.0,
                ..Crop::default()
            },
            scale: Scale::P720,
            rotate: Rotate::Cw90,
            flip_h: true,
            ..VideoTransform::default()
        },
        ..ExportOptions::default()
    };
    let filters = options.video_filters().unwrap();
    let crop = filters.find("crop=").unwrap();
    let scale = filters.find("scale=-2:720").unwrap();
    let rotate = filters.find("transpose=1").unwrap();
    let flip = filters.find("hflip").unwrap();
    let denoise = filters.find("hqdn3d").unwrap();
    assert!(crop < scale && scale < rotate && rotate < flip);
    assert!(flip < denoise, "几何变换要排在降噪之前");
}

#[test]
fn 采样率与声道只在重编码音频时写入() {
    let options = ExportOptions {
        sample_rate: Some(44_100),
        channels: Some(1),
        ..ExportOptions::default()
    };
    let encoded = args(options);
    assert!(has(&encoded, &["-ar", "44100"]));
    assert!(has(&encoded, &["-ac", "1"]));

    // 复制音频时样本根本不解出来，这两个参数也就没有意义。
    let copy = ExportOptions {
        audio_copy: true,
        sample_rate: Some(44_100),
        channels: Some(1),
        ..ExportOptions::default()
    };
    let copied = args(copy);
    assert!(!copied.iter().any(|arg| arg == "-ar"));
    assert!(!copied.iter().any(|arg| arg == "-ac"));
}

#[test]
fn 音量增益排在响度标准化之后() {
    let options = ExportOptions {
        normalize_audio: true,
        volume_db: 3.0,
        ..ExportOptions::default()
    };
    let filters = options.audio_filters().unwrap();
    assert!(
        filters.find("loudnorm").unwrap() < filters.find("volume=3.0dB").unwrap(),
        "增益要作用在标准化之后，否则会被拉回去：{filters}"
    );
}
