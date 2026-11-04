use super::*;

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
fn hls_fmp4_写分片类型与初始化段名() {
    let options = ExportOptions {
        format: OutputFormat::HlsFmp4,
        ..ExportOptions::default()
    };
    let args = args(options);
    assert!(has(&args, &["-f", "hls"]));
    assert!(has(&args, &["-hls_segment_type", "fmp4"]));
    assert!(has(
        &args,
        &["-hls_fmp4_init_filename", "movie - 片段_init.mp4"]
    ));
    assert!(has(
        &args,
        &["-hls_segment_filename", "movie - 片段_%04d.m4s"]
    ));
    assert!(args.last().unwrap().ends_with(".m3u8"));
}

#[test]
fn dash_输出_mpd_与_m4s_分片() {
    let options = ExportOptions {
        format: OutputFormat::Dash,
        ..ExportOptions::default()
    };
    let args = args(options);
    assert!(has(&args, &["-f", "dash"]));
    assert!(has(&args, &["-seg_duration", "6"]));
    assert!(has(
        &args,
        &["-init_seg_name", "movie - 片段_init$RepresentationID$.m4s"]
    ));
    assert!(has(
        &args,
        &[
            "-media_seg_name",
            "movie - 片段_$RepresentationID$_$Number%05d$.m4s"
        ]
    ));
    // DASH 与 HLS 一样不带字幕/章节，也固定 8bit。
    assert!(has(&args, &["-map_chapters", "-1"]));
    assert!(!has(&args, &["-map", "0:s?"]));
    assert!(has(&args, &["-pix_fmt", "yuv420p"]));
    assert!(args.last().unwrap().ends_with(".mpd"));
}

#[test]
fn 按大小切分时第一遍只出单文件() {
    let options = ExportOptions {
        format: OutputFormat::Split,
        split_mb: 300,
        ..ExportOptions::default()
    };
    let args = args(options);
    // 切分交给第二遍流复制：这里必须是普通 mkv，不能写成 segment muxer。
    assert_eq!(args.last().unwrap(), "movie - 片段.mkv");
    assert!(!has(&args, &["-f", "segment"]));
    // 关键帧密一点，第二遍切出来的段才接近目标大小。
    assert!(has(&args, &["-force_key_frames", "expr:gte(t,n_forced*6)"]));
}

#[test]
fn 切分用的临时文件与分片模板() {
    let output = Path::new("movie - 片段.mkv");
    assert_eq!(
        super::split_temp_path(output),
        Path::new("movie - 片段.part.mkv")
    );
    assert_eq!(
        super::split_pattern(output),
        Path::new("movie - 片段_%03d.mkv")
    );
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
