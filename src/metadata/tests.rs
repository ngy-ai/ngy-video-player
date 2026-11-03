use super::*;

fn request(chapters: Vec<ChapterEdit>) -> MetadataRequest {
    MetadataRequest {
        input: PathBuf::from("movie.mp4"),
        output: PathBuf::from("movie - 元数据.mkv"),
        duration: 600.0,
        tags: Tags {
            title: "新标题".into(),
            artist: "作者".into(),
            comment: String::new(),
        },
        chapters,
        subtitle_codecs: vec!["mov_text".into()],
        cover: None,
    }
}

fn args(request: &MetadataRequest, chapters: Option<&Path>) -> Vec<String> {
    // 测试里的输入没有附件，新封面的序号就是 0。
    build_command(
        Path::new("ffmpeg"),
        request,
        chapters,
        0,
        request.cover.as_deref(),
    )
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

#[test]
fn 章节按时间排序去重并补默认标题() {
    let chapters = normalize_chapters(
        vec![
            ChapterEdit {
                title: "  ".into(),
                start: 30.0,
            },
            ChapterEdit {
                title: "开场".into(),
                start: 0.0,
            },
            ChapterEdit {
                title: "重复".into(),
                start: 30.05,
            },
            ChapterEdit {
                title: "越界".into(),
                start: 600.0,
            },
        ],
        600.0,
    );
    assert_eq!(chapters.len(), 2);
    assert_eq!(chapters[0].title, "开场");
    assert_eq!(chapters[1].title, "章节 2");
    assert_eq!(chapters[1].start, 30.0);
}

#[test]
fn 章节文本用毫秒并转义语法字符() {
    let text = chapters_text(
        &[
            ChapterEdit {
                title: "a=b;c#d\\e".into(),
                start: 1.5,
            },
            ChapterEdit {
                title: "结尾".into(),
                start: 10.0,
            },
        ],
        12.0,
    );
    assert!(text.starts_with(";FFMETADATA1\n"));
    assert!(text.contains("START=1500\nEND=10000\n"));
    assert!(text.contains("title=a\\=b\\;c\\#d\\\\e\n"));
    // 最后一条的 END 取总时长，不能是 0。
    assert!(text.contains("START=10000\nEND=12000\n"));
}

#[test]
fn 命令流复制并写入三项标签() {
    let args = args(&request(Vec::new()), None);
    assert!(has(&args, &["-c", "copy"]));
    assert!(has(&args, &["-metadata", "title=新标题"]));
    assert!(has(&args, &["-metadata", "artist=作者"]));
    assert!(has(&args, &["-metadata", "comment="]));
    assert!(has(&args, &["-c:s", "srt"]), "mov_text 要转成 srt");
    assert!(!has(&args, &["-i", "chapters.txt"]));
}

#[test]
fn 封面图作为附件写进输出() {
    let mut with_cover = request(Vec::new());
    with_cover.cover = Some(PathBuf::from("cover.jpg"));
    let with_cover = args(&with_cover, None);
    assert!(has(&with_cover, &["-attach", "cover.jpg"]));
    // mkv 的附件必须带 mimetype，否则 muxer 拒绝写头。
    assert!(has(
        &with_cover,
        &["-metadata:s:t:0", "mimetype=image/jpeg"]
    ));
    assert!(has(&with_cover, &["-metadata:s:t:0", "filename=cover.jpg"]));
    // 现有的附件仍然原样带过去，封面只是多一条。
    assert!(has(&with_cover, &["-map", "0:t?"]));
    assert!(has(&with_cover, &["-c", "copy"]));

    // 没设封面时不该凭空多一个附件。
    let plain = args(&request(Vec::new()), None);
    assert!(!plain.iter().any(|arg| arg == "-attach"));
}

#[test]
fn 原文件既有附件又有封面时两样都认出来() {
    let text = "\
Input #0, matroska,webm, from 'in.mkv':
  Stream #0:0: Video: h264 (High), yuv420p(progressive), 160x120, 25 fps
  Stream #0:1(jpn): Audio: aac (LC), 44100 Hz, mono, fltp
  Stream #0:2(swe): Subtitle: ass (ssa)
  Stream #0:3: Video: mjpeg (Baseline), yuvj420p(pc), 120x120, 90k tbr (attached pic)
  Stream #0:4: Attachment: ttf
";
    let probe = parse_probe(text);
    // 附件数决定新封面的 `-metadata:s:t:N` 序号，封面自己不算附件。
    assert_eq!(probe.attachments, 1);
    let cover = probe.cover.expect("没认出封面");
    // 封面是第二条视频流（主视频是 0），提取时要按视频流内的序号取。
    assert_eq!(cover.video_index, 1);
    assert_eq!(cover.extension, "jpg");

    // 没有封面的普通文件不该凭空多出一张封面。
    let plain = parse_probe("  Stream #0:0: Video: h264, yuv420p\n");
    assert!(plain.cover.is_none());
    assert_eq!(plain.attachments, 0);
}

#[test]
fn 封面扩展名按编码推() {
    assert_eq!(
        cover_extension("  Stream #0:3: Video: png, rgba(pc), 120x120"),
        "png"
    );
    assert_eq!(
        cover_extension("  Stream #0:3: Video: mjpeg (Baseline), yuvj420p(pc)"),
        "jpg"
    );
    // 认不出的编码退回 jpg：提取失败时按「没有封面」处理，不影响保存。
    assert_eq!(cover_extension("  Stream #0:3: Video: weird"), "jpg");
}

#[test]
fn 没有章节时移除原章节() {
    let args = args(&request(Vec::new()), None);
    assert!(has(&args, &["-map_chapters", "-1"]));
}

#[test]
fn 有章节时从第二个输入取章节() {
    let chapters = PathBuf::from("chapters.txt");
    let args = args(
        &request(vec![ChapterEdit {
            title: "一".into(),
            start: 1.0,
        }]),
        Some(&chapters),
    );
    assert!(has(&args, &["-i", "chapters.txt"]));
    assert!(has(&args, &["-map_chapters", "1"]));
    // 章节输入不能被当成媒体流映射进输出。
    assert!(!has(&args, &["-map", "1"]));
}

#[test]
fn 输出文件不覆盖已有文件() {
    let dir = std::env::temp_dir().join(format!("ngy-metadata-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let input = dir.join("课程 01.mp4");
    std::fs::write(&input, b"x").unwrap();

    let first = output_path(&input);
    assert_eq!(first, dir.join("课程 01 - 元数据.mkv"));
    std::fs::write(&first, b"x").unwrap();

    let second = output_path(&input);
    assert_eq!(second, dir.join("课程 01 - 元数据 (2).mkv"));

    std::fs::remove_dir_all(&dir).unwrap();
}
