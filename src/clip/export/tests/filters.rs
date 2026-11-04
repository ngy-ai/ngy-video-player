use super::*;

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
fn 烧录字幕时不再复制软字幕() {
    let subtitle = Path::new("C:/Users/me/字幕.ass");
    let args = command_args(ExportOptions::default(), Some(subtitle));
    assert!(!has(&args, &["-map", "0:s?"]), "烧录后不该再映射软字幕");
    assert!(!args.iter().any(|arg| arg == "-c:s"));

    let index = args.iter().position(|arg| arg == "-vf").expect("要有滤镜");
    let filters = &args[index + 1];
    // 盘符冒号必须转义，否则 ffmpeg 会把它当成滤镜参数的冒号分隔符。
    assert!(
        filters.contains("subtitles=filename='C\\:/Users/me/字幕.ass'"),
        "{filters}"
    );
}

#[test]
fn 烧录字幕排在画面处理之后_gif_调色板之前() {
    let options = ExportOptions {
        format: OutputFormat::Gif,
        sharpen: true,
        ..ExportOptions::default()
    };
    let args = command_args(options, Some(Path::new("/tmp/a.ass")));
    let index = args.iter().position(|arg| arg == "-vf").expect("要有滤镜");
    let filters = &args[index + 1];
    let sharpen = filters.find("unsharp").unwrap();
    let subtitles = filters.find("subtitles=").unwrap();
    let palette = filters.find("palettegen").unwrap();
    assert!(sharpen < subtitles, "字幕画在最终画面上：{filters}");
    assert!(subtitles < palette, "调色板必须排在最后：{filters}");
}

#[test]
fn 色彩转换合成一条_zscale_并排在几何变换之前() {
    let options = ExportOptions {
        color_matrix: ColorMatrix::Bt709,
        color_range: ColorRange::Tv,
        transform: VideoTransform {
            scale: Scale::P720,
            ..VideoTransform::default()
        },
        ..ExportOptions::default()
    };
    let filters = options.video_filters().unwrap();
    assert!(
        filters.contains("zscale=m=bt709:p=bt709:t=bt709:range=limited"),
        "{filters}"
    );
    let zscale = filters.find("zscale").unwrap();
    let scale = filters.find("scale=").unwrap();
    assert!(zscale < scale, "先统一色彩再缩放：{filters}");

    // 跟随源时不该多出任何色彩滤镜。
    let plain = ExportOptions::default().video_filters();
    assert!(plain.is_none_or(|filters| !filters.contains("zscale")));
}

#[test]
fn 重写色彩标签写进输出参数() {
    let args = args(ExportOptions {
        tag_rec709: true,
        ..ExportOptions::default()
    });
    assert!(has(&args, &["-color_primaries", "bt709"]));
    assert!(has(&args, &["-color_trc", "bt709"]));
    assert!(has(&args, &["-colorspace", "bt709"]));
}

#[test]
fn 倒放同时反转画面与声音() {
    let options = ExportOptions {
        reverse: true,
        ..ExportOptions::default()
    };
    let filters = options.video_filters().unwrap();
    assert!(
        filters.split(',').any(|filter| filter == "reverse"),
        "{filters}"
    );
    let audio = options.audio_filters().unwrap();
    assert!(
        audio.split(',').any(|filter| filter == "areverse"),
        "{audio}"
    );
}

#[test]
fn 倒放压过音频复制() {
    // `areverse` 需要解码后的样本，`-c:a copy` 下它会静默失效，
    // 结果是画面倒放、声音正放——比不勾更糟。
    let options = ExportOptions {
        reverse: true,
        audio_copy: true,
        ..ExportOptions::default()
    };
    assert!(!options.copies_audio());
    let args = args(options);
    assert!(has(&args, &["-c:a", "aac"]));
    assert!(!has(&args, &["-c:a", "copy"]));
}

#[test]
fn 开头定格补画面并把声音推迟() {
    let options = ExportOptions {
        freeze_lead: 2.0,
        ..ExportOptions::default()
    };
    let filters = options.video_filters().unwrap();
    assert!(
        filters.contains("tpad=start_mode=clone:start_duration=2.000"),
        "{filters}"
    );
    let audio = options.audio_filters().unwrap();
    assert!(audio.contains("adelay=2000:all=1"), "{audio}");

    // -t 必须按加长后的时长写：按原区间写会把定格那段直接截掉。
    let options = ExportOptions {
        freeze_lead: 2.0,
        ..ExportOptions::default()
    };
    let args = args(options);
    assert!(has(&args, &["-t", "17.000"]), "{args:?}");
}

#[test]
fn 定格时长被夹到量程内() {
    let options = ExportOptions {
        freeze_lead: -3.0,
        ..ExportOptions::default()
    };
    assert_eq!(options.freeze_seconds(), 0.0);
    assert_eq!(options.video_filters(), None);

    let too_long = ExportOptions {
        freeze_lead: 900.0,
        ..ExportOptions::default()
    };
    assert_eq!(too_long.freeze_seconds(), MAX_FREEZE_SECONDS);
}

#[test]
fn 水印画在几何变换之后() {
    let options = ExportOptions {
        sharpen: true,
        watermark: Watermark {
            enabled: true,
            text: "示例台标".into(),
            position: WatermarkPosition::TopRight,
            size: 32,
            opacity: 0.6,
        },
        ..ExportOptions::default()
    };
    let filters = options.video_filters().unwrap();
    let unsharp = filters.find("unsharp").unwrap();
    let drawtext = filters.find("drawtext").unwrap();
    assert!(unsharp < drawtext, "水印要按最终画面定位：{filters}");
    assert!(filters.contains("text='示例台标'"), "{filters}");
    assert!(filters.contains("x=w-tw-24:y=24"), "{filters}");
}

#[test]
fn 防抖排在降噪之前_插帧排在逐帧处理之后() {
    let options = ExportOptions {
        stabilize: true,
        denoise: true,
        sharpen: true,
        interpolate: true,
        watermark: Watermark {
            enabled: true,
            text: "台标".into(),
            ..Watermark::default()
        },
        ..ExportOptions::default()
    };
    let filters = options.video_filters().unwrap();
    let deshake = filters.find("deshake=").unwrap();
    let denoise = filters.find("hqdn3d=").unwrap();
    let sharpen = filters.find("unsharp=").unwrap();
    let interpolate = filters.find("minterpolate=").unwrap();
    let watermark = filters.find("drawtext=").unwrap();
    assert!(deshake < denoise, "{filters}");
    assert!(denoise < sharpen, "{filters}");
    assert!(sharpen < interpolate, "{filters}");
    assert!(interpolate < watermark, "水印不该被插帧抹出残影：{filters}");
}

#[test]
fn 插帧按选定的目标帧率写() {
    let options = ExportOptions {
        interpolate: true,
        interpolate_fps: FrameRate::F120,
        ..ExportOptions::default()
    };
    let filters = options.video_filters().unwrap();
    assert!(filters.contains("minterpolate=fps=120"), "{filters}");
}

#[test]
fn 流复制时倒放与定格都不落地() {
    let options = ExportOptions {
        mode: ExportMode::Copy,
        reverse: true,
        freeze_lead: 3.0,
        ..ExportOptions::default()
    };
    assert_eq!(options.video_filters(), None);
    assert_eq!(options.audio_filters(), None);
}

#[test]
fn 两遍响度把测量值喂回滤镜() {
    let options = ExportOptions {
        normalize_audio: true,
        two_pass_loudness: true,
        ..ExportOptions::default()
    };
    let loudness = Loudness {
        input_i: -23.06,
        input_tp: -6.02,
        input_lra: 7.2,
        input_thresh: -33.1,
        target_offset: 0.01,
    };
    let filters = options.audio_filters_with(Some(&loudness)).unwrap();
    assert!(filters.contains("measured_I=-23.06"), "{filters}");
    assert!(filters.contains("offset=0.01"), "{filters}");
    assert!(filters.contains("linear=true"), "{filters}");

    // 第一遍没跑成时退回单遍，不能拼出半截参数。
    assert_eq!(
        options.audio_filters().as_deref(),
        Some("loudnorm=I=-16:TP=-1.5:LRA=11")
    );
}

#[test]
fn 两遍响度只在真重编码音频时才做() {
    let copied = ExportOptions {
        normalize_audio: true,
        two_pass_loudness: true,
        audio_copy: true,
        ..ExportOptions::default()
    };
    assert!(!copied.wants_two_pass_loudness(), "复制码流不解码音频");

    let off = ExportOptions {
        normalize_audio: false,
        two_pass_loudness: true,
        ..ExportOptions::default()
    };
    assert!(!off.wants_two_pass_loudness(), "没开标准化就没有测量对象");
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
