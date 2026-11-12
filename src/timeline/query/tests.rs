use super::*;
use crate::timeline::{AssetId, MediaStamp, Rational, Tick};

/// 造一条素材：名字、标签、时长（秒）、尺寸、编码、修改日期（Unix 秒）。
fn asset(
    name: &str,
    tags: &[&str],
    seconds: f64,
    (width, height): (u32, u32),
    codec: &str,
    modified: i64,
) -> Asset {
    let path = std::path::PathBuf::from(name);
    Asset {
        id: AssetId::new(1),
        path: path.clone(),
        resolved: path,
        missing: false,
        media: Some(MediaInfo {
            duration: Tick::from_secs(seconds),
            fps: Some(Rational::FPS_25),
            width,
            height,
            video_codec: codec.to_string(),
            audio_codec: "aac".to_string(),
            ..MediaInfo::default()
        }),
        stamp: Some(MediaStamp { size: 1, modified }),
        tags: tags.iter().map(|tag| tag.to_string()).collect(),
    }
}

fn hits(text: &str, asset: &Asset) -> bool {
    let (query, unknown) = Query::parse(text);
    assert!(unknown.is_empty(), "{text} 里有认不出来的条件：{unknown:?}");
    query.matches(asset)
}

#[test]
fn 裸词只搜名称() {
    let clip = asset("航拍 日出.mp4", &[], 10.0, (1920, 1080), "h264", 0);
    assert!(hits("航拍", &clip));
    // 大小写不敏感：文件名常见 `DJI_0001.MP4`。
    assert!(hits(
        "dji_0001",
        &asset("DJI_0001.MP4", &[], 1.0, (1, 1), "h264", 0)
    ));
    assert!(!hits("日落", &clip));
}

#[test]
fn 多个空格条件之间是并且() {
    let clip = asset("航拍 日出.mp4", &["航拍"], 90.0, (3840, 2160), "h264", 0);
    assert!(hits("名称:航拍 时长>60", &clip));
    // 一条不满足就不命中。
    assert!(!hits("名称:航拍 时长>120", &clip));
    assert!(!hits("名称:日落 时长>60", &clip));
}

#[test]
fn 短横线是否定() {
    let clip = asset(
        "航拍 日落.mp4",
        &["航拍", "废片"],
        30.0,
        (1920, 1080),
        "h264",
        0,
    );
    assert!(hits("航拍", &clip));
    assert!(!hits("-航拍", &clip));
    // 否定标签：带「废片」的不要。
    assert!(!hits("-标签:废片", &clip));
    assert!(hits("-标签:成片", &clip));
}

#[test]
fn 标签是整词匹配() {
    let clip = asset("a.mp4", &["航拍", "空镜"], 10.0, (1920, 1080), "h264", 0);
    assert!(hits("标签:航拍", &clip));
    assert!(hits("tag:空镜", &clip));
    // 不做子串匹配：「航」不等于「航拍」。
    assert!(!hits("标签:航", &clip));
}

#[test]
fn 时长与分辨率比大小() {
    let clip = asset("a.mp4", &[], 90.0, (1920, 1080), "h264", 0);
    assert!(hits("时长>=90", &clip));
    assert!(hits("时长<=90", &clip));
    assert!(!hits("时长>90", &clip));
    assert!(!hits("时长<90", &clip));
    // 分辨率按长边：1080p 素材既算 1920 也算 >=1080。
    assert!(hits("分辨率>=1920", &clip));
    assert!(!hits("分辨率>1920", &clip));
    // 数字写坏了：不命中，而不是当成 0 命中。
    assert!(!hits("时长>abc", &clip));
}

#[test]
fn 帧率与编码() {
    let clip = asset("a.mp4", &[], 10.0, (1920, 1080), "hevc", 0);
    assert!(hits("帧率>=25", &clip));
    assert!(!hits("帧率>25", &clip));
    assert!(hits("编码:hevc", &clip));
    assert!(!hits("编码:h264", &clip));
}

#[test]
fn 日期按天比() {
    // 2026-11-12 00:00:00 UTC（当年第 316 天）
    let day = 1_794_441_600i64;
    let clip = asset("a.mp4", &[], 10.0, (1920, 1080), "h264", day);
    assert!(hits("日期>2026-11-01", &clip));
    assert!(!hits("日期<2026-11-01", &clip));
    assert!(hits("日期>=2026-11-12", &clip));
    assert!(!hits("日期>2026-11-12", &clip));
    // 斜杠与点也认。
    assert!(hits("日期>=2026/11/01", &clip));
}

#[test]
fn 年月日换算对了() {
    assert_eq!(day_from_unix(0), (1970, 1, 1));
    assert_eq!(day_from_unix(86_400 * 365), (1971, 1, 1));
    // 2000 是闰年：2 月 29 日存在，且 366 天到下一年的 1 月 1 日。
    assert_eq!(day_from_unix(951_782_400), (2000, 2, 29));
    // 2000-02-29 再过 307 天就是 2001-01-01。
    assert_eq!(day_from_unix(951_782_400 + 86_400 * 307), (2001, 1, 1));
    assert_eq!(day_from_unix(1_794_441_600), (2026, 11, 12));
}

#[test]
fn 认不出来的条件要报出来() {
    // 裸词是合法的名称条件；`时长>` 这种切不出值的才算写坏。
    let (query, unknown) = Query::parse("名称:航拍 瞎写的一坨 时长>");
    assert_eq!(query.terms.len(), 2, "名称 + 裸词");
    assert_eq!(unknown, vec!["时长>".to_string()]);
}

#[test]
fn 空查询命中全部() {
    let (query, unknown) = Query::parse("   ");
    assert!(unknown.is_empty());
    assert!(query.is_empty());
    let clip = asset("a.mp4", &[], 1.0, (1, 1), "h264", 0);
    assert!(query.matches(&clip));
}

#[test]
fn 合集存条件而不是结果() {
    // 同一个合集串，作用在不同素材上结果不同：说明它没把 id 存进去。
    let collection = Collection::new("长镜头", "时长>=60 标签:空镜");
    let (query, _) = Query::parse(&collection.query);
    let long = asset("long.mp4", &["空镜"], 90.0, (1920, 1080), "h264", 0);
    let short = asset("short.mp4", &["空镜"], 10.0, (1920, 1080), "h264", 0);
    assert!(query.matches(&long));
    assert!(!query.matches(&short));

    // 存盘再读回来，条件还在。
    let text = serde_json::to_string(&collection).expect("序列化");
    let back: Collection = serde_json::from_str(&text).expect("反序列化");
    assert_eq!(back, collection);
}

#[test]
fn 工程里的标签与合集能存盘读回() {
    let mut project = crate::timeline::Project::default();
    let dir = std::env::temp_dir();
    let path = dir.join("ngy-query.ngyproj");
    let id = project.add_asset(std::path::Path::new("C:/素材/a.mp4"));
    // 素材表是公开字段：直接改那一行，不必为测试开一个 setter。
    if let Some(asset) = project.assets.iter_mut().find(|asset| asset.id == id) {
        asset.tags = vec!["航拍".to_string()];
    }
    project
        .collections
        .push(Collection::new("航拍", "标签:航拍"));
    project.save(&path).expect("保存");

    let opened = crate::timeline::Project::open(&path).expect("打开");
    let asset = opened
        .assets
        .iter()
        .find(|asset| asset.id == id)
        .expect("素材还在");
    assert_eq!(asset.tags, vec!["航拍".to_string()]);
    assert_eq!(opened.collections.len(), 1);
    assert_eq!(opened.collections[0].query, "标签:航拍");
    let _ = std::fs::remove_file(&path);
}
