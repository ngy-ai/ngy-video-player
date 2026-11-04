use super::*;

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
        None,
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
fn 选_h265_时换编码器() {
    let options = ExportOptions {
        codec: VideoCodec::Hevc,
        ..ExportOptions::default()
    };
    let args = args(options);
    assert!(has(&args, &["-c:v", "libx265"]));
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
fn 自定义_crf_替换画质档() {
    let options = ExportOptions {
        quality: Quality::High,
        crf: Some(28),
        ..ExportOptions::default()
    };
    let args = args(options);
    assert!(has(&args, &["-crf", "28"]));
    assert!(!has(&args, &["-crf", "18"]), "画质档被自定义值覆盖");
}

#[test]
fn 固定码率时不再写_crf() {
    let options = ExportOptions {
        rate_control: RateControl::Bitrate,
        bitrate_kbps: 8_000,
        ..ExportOptions::default()
    };
    let args = args(options);
    assert!(has(&args, &["-maxrate", "8000k"]));
    assert!(has(&args, &["-bufsize", "16000k"]));
    assert!(!args.iter().any(|arg| arg == "-crf"));
}
