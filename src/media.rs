//! 素材探测：跑一次 `ffmpeg -i`，把时间线要的元信息一次读完。
//!
//! 不借播放中的 libmpv 实例：探测要 `loadfile`，会把用户正在看的片子顶掉
//! （`player/probe.rs` 的注释里说过同一件事）。这里只读 stderr 里的文件头，
//! 不解码、不出声、不写临时文件。
//!
//! 探测结果写进 [`Project`] 的素材表，并附上文件指纹（大小 + 修改时间）。
//! 文件被换过时 [`Asset::state`] 会报 `Changed`，界面提示重链，而不是拿旧时长
//! 继续剪——那会让所有切点整体错位。

use crate::platform::ffmpeg;
use crate::timeline::{
    AssetId, AssetState, ColorTags, MediaInfo, MediaStamp, Project, Rational, Tick,
};
use anyhow::{Context as _, Result, anyhow, bail};
use std::path::Path;
use std::process::{Command, Stdio};

/// 探测一个文件。只读文件头，不解码。
pub fn probe(path: &Path) -> Result<MediaInfo> {
    if !path.is_file() {
        bail!("文件已不存在：{}", path.display());
    }
    let executable = ffmpeg::executable()?;
    let output = Command::new(&executable)
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("启动 ffmpeg 探测失败：{}", executable.display()))?;

    // 没给输出文件时 ffmpeg 一定会以非零码退出，文件信息仍然打在 stderr 上，
    // 所以这里不看退出码，只解析文本。
    let text = String::from_utf8_lossy(&output.stderr);
    parse(&text).ok_or_else(|| anyhow!("读不出素材信息：{}", path.display()))
}

/// 登记素材并探测。已有可信缓存时不再重探。
pub fn register(project: &mut Project, path: &Path) -> Result<AssetId> {
    let id = project.add_asset(path);
    if let Some(asset) = project.asset(id)
        && asset.media.is_some()
        && asset.state() == AssetState::Ready
    {
        return Ok(id);
    }

    let info = probe(path)?;
    let stamp =
        MediaStamp::of(path).ok_or_else(|| anyhow!("读不到文件指纹：{}", path.display()))?;
    let asset = project
        .assets
        .iter_mut()
        .find(|asset| asset.id == id)
        .ok_or_else(|| anyhow!("素材登记失败：{}", path.display()))?;
    asset.media = Some(info);
    asset.stamp = Some(stamp);
    asset.missing = false;
    Ok(id)
}

/// 从 `ffmpeg -i` 的 stderr 里解析元信息。解析不出来（没有 Duration 也没有轨道）返回 `None`。
///
/// 单独拆出来是为了能用真实的 ffmpeg 输出做单测——跑一遍 ffmpeg 的测试要标 `#[ignore]`，
/// 而解析逻辑本身必须天天跑。
pub(crate) fn parse(text: &str) -> Option<MediaInfo> {
    let duration = ffmpeg::parse_duration_line(text)?;
    let mut info = MediaInfo {
        duration: Tick::from_secs(duration),
        rotation: parse_rotation(text),
        ..MediaInfo::default()
    };

    let mut audio_seen = false;
    for line in text.lines() {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix("Stream #") else {
            continue;
        };
        if let Some((_, video)) = rest.split_once("Video: ") {
            parse_video(video, &mut info);
        } else if !audio_seen && let Some((_, audio)) = rest.split_once("Audio: ") {
            // 多音轨素材只记第一条：时间线上一个片段默认用第一条音轨，
            // 换轨由轨道编辑（remux）负责，不在这里做。
            parse_audio(audio, &mut info);
            audio_seen = true;
        }
    }

    // 只有字幕、没有音视频轨的文件也当探测失败：时间线放不进去。
    if !info.has_video() && !info.has_audio() {
        return None;
    }
    Some(info)
}

fn parse_video(rest: &str, info: &mut MediaInfo) {
    info.video_codec = first_token(rest);
    if let Some((pixel_format, color)) = find_pixel_format(rest) {
        info.pixel_format = pixel_format;
        info.color = color;
    }

    for field in rest.split(',') {
        let field = field.trim();
        if info.fps.is_none()
            && let Some(number) = field.strip_suffix(" fps")
            && let Ok(fps) = number.trim().parse::<f64>()
        {
            info.fps = Rational::from_f64(fps);
        }
        if info.width == 0
            && let Some((width, height)) = parse_size(field)
        {
            info.width = width;
            info.height = height;
        }
    }
}

fn parse_audio(rest: &str, info: &mut MediaInfo) {
    info.audio_codec = first_token(rest);
    for field in rest.split(',').map(str::trim) {
        if let Some(rate) = field.strip_suffix(" Hz")
            && let Ok(rate) = rate.trim().parse::<u32>()
        {
            info.sample_rate = rate;
            continue;
        }
        if info.channels == 0
            && let Some(channels) = layout_channels(field)
        {
            info.channels = channels;
        }
    }
}

/// `h264 (High) (avc1 / 0x31637661)` → `h264`。
fn first_token(text: &str) -> String {
    text.split(|ch: char| ch.is_whitespace() || ch == ',' || ch == '(')
        .find(|token| !token.is_empty())
        .unwrap_or("")
        .to_string()
}

/// `1920x1080 [SAR 1:1 DAR 16:9]` → `(1920, 1080)`。
fn parse_size(text: &str) -> Option<(u32, u32)> {
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if !chars[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        let width_start = index;
        while index < chars.len() && chars[index].is_ascii_digit() {
            index += 1;
        }
        if index < chars.len()
            && (chars[index] == 'x' || chars[index] == 'X')
            && index + 1 < chars.len()
            && chars[index + 1].is_ascii_digit()
        {
            let width: String = chars[width_start..index].iter().collect();
            let mut end = index + 1;
            while end < chars.len() && chars[end].is_ascii_digit() {
                end += 1;
            }
            let height: String = chars[index + 1..end].iter().collect();
            if let (Ok(width), Ok(height)) = (width.parse(), height.parse()) {
                return Some((width, height));
            }
        }
    }
    None
}

/// 在 `yuv420p(tv, bt709, progressive)` 里找出像素格式与括号里的色彩标签。
fn find_pixel_format(text: &str) -> Option<(String, ColorTags)> {
    let mut index = 0;
    while index < text.len() {
        if !text.is_char_boundary(index) {
            index += 1;
            continue;
        }
        let before = text[..index].chars().next_back();
        let boundary = index == 0 || matches!(before, Some(' ' | ',' | ':'));
        if boundary {
            let tail = &text[index..];
            let name: String = tail
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                .collect();
            if is_pixel_format(&name) {
                let color = tail[name.len()..]
                    .strip_prefix('(')
                    .and_then(|rest| rest.split_once(')'))
                    .map(|(inner, _)| parse_color(inner))
                    .unwrap_or_default();
                return Some((name, color));
            }
        }
        index += text[index..].chars().next().map_or(1, char::len_utf8);
    }
    None
}

fn is_pixel_format(name: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "yuv", "yuva", "yuvj", "rgb", "rgba", "gbr", "gbrap", "gray", "ya", "nv", "p0", "p1", "p2",
        "p4",
    ];
    name.len() >= 3 && PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

fn parse_color(inner: &str) -> ColorTags {
    let mut parts = inner.split(',').map(str::trim);
    let range = parts.next().unwrap_or_default().to_string();
    let tags: Vec<&str> = parts
        .filter(|part| !part.is_empty() && *part != "progressive")
        .collect();
    ColorTags {
        range,
        tags: tags.join("/"),
    }
}

/// `stereo` / `mono` / `5.1(side)` / `2 channels` → 声道数。
fn layout_channels(field: &str) -> Option<u32> {
    let field = field.trim();
    if field == "mono" {
        return Some(1);
    }
    if field == "stereo" {
        return Some(2);
    }
    if let Some(count) = field.strip_suffix(" channels") {
        return count.trim().parse().ok();
    }
    // 带空格的字段是 "128 kb/s" 之类，不是声道布局。
    if field.contains(' ') {
        return None;
    }
    let head: String = field
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect();
    if head.is_empty() {
        return None;
    }
    let mut parts = head.split('.');
    let main: u32 = parts.next()?.parse().ok()?;
    let lfe: u32 = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    Some(main + lfe)
}

/// `displaymatrix: rotation of -90.00 degrees` → 270。
fn parse_rotation(text: &str) -> u16 {
    const KEY: &str = "rotation of ";
    let Some(index) = text.find(KEY) else {
        return 0;
    };
    let rest = &text[index + KEY.len()..];
    let number: String = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '-' || *ch == '.')
        .collect();
    let Ok(value) = number.parse::<f64>() else {
        return 0;
    };
    let mut rotation = value.round() as i64 % 360;
    if rotation < 0 {
        rotation += 360;
    }
    rotation as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    // 下面几段是 `ffmpeg -i` 的真实输出节选（版本不同会多几个字段，
    // 解析只认自己需要的，不假设字段顺序）。
    const MP4: &str = "\
Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'a.mp4':
  Metadata:
    major_brand     : isom
  Duration: 00:00:10.02, start: 0.000000, bitrate: 1234 kb/s
  Stream #0:0[0x1](und): Video: h264 (High) (avc1 / 0x31637661), yuv420p(tv, bt709, progressive), 1920x1080 [SAR 1:1 DAR 16:9], 5000 kb/s, 29.97 fps, 29.97 tbr, 15360 tbn (default)
  Stream #0:1[0x2](und): Audio: aac (LC) (mp4a / 0x6134706D), 48000 Hz, stereo, fltp, 128 kb/s (default)
";

    const HDR_MKV: &str = "\
  Duration: 00:01:00.00, start: 0.000000, bitrate: 20000 kb/s
  Stream #0:0: Video: hevc (Main 10), yuv420p10le(tv, bt2020nc/bt2020/smpte2084, progressive), 3840x2160, 24 fps, 24 tbr, 1k tbn
  Stream #0:1: Audio: eac3, 48000 Hz, 5.1(side), fltp, 640 kb/s
";

    const PORTRAIT: &str = "\
  Duration: 00:00:05.00, start: 0.000000, bitrate: 8000 kb/s
  Stream #0:0[0x1](und): Video: h264 (High), yuv420p(tv, bt709, progressive), 1920x1080, 30 fps, 30 tbr, 90k tbn
    Side data:
      displaymatrix: rotation of -90.00 degrees
  Stream #0:1[0x2](und): Audio: aac (LC), 44100 Hz, mono, fltp, 96 kb/s
";

    const AUDIO_ONLY: &str = "\
  Duration: 00:03:20.00, start: 0.000000, bitrate: 320 kb/s
  Stream #0:0: Audio: mp3, 44100 Hz, stereo, fltp, 320 kb/s
";

    #[test]
    fn 解析_h264_与_aac() {
        let info = parse(MP4).expect("能解析");
        assert_eq!(info.duration, Tick::from_secs(10.02));
        assert_eq!(info.fps, Some(Rational::FPS_29_97));
        assert_eq!((info.width, info.height), (1920, 1080));
        assert_eq!(info.pixel_format, "yuv420p");
        assert_eq!(info.color.range, "tv");
        assert_eq!(info.color.tags, "bt709");
        assert_eq!(info.video_codec, "h264");
        assert_eq!(info.audio_codec, "aac");
        assert_eq!(info.channels, 2);
        assert_eq!(info.sample_rate, 48_000);
        assert_eq!(info.rotation, 0);
        assert_eq!(info.display_size(), (1920, 1080));
    }

    #[test]
    fn 解析_hdr_十位与_5_1_声道() {
        let info = parse(HDR_MKV).expect("能解析");
        assert_eq!(info.pixel_format, "yuv420p10le");
        assert_eq!(info.color.range, "tv");
        assert_eq!(info.color.tags, "bt2020nc/bt2020/smpte2084");
        assert_eq!((info.width, info.height), (3840, 2160));
        assert_eq!(info.fps, Some(Rational::FPS_24));
        assert_eq!(info.channels, 6, "5.1 是六个声道");
    }

    #[test]
    fn 手机竖拍按旋转元数据换算() {
        let info = parse(PORTRAIT).expect("能解析");
        assert_eq!(info.rotation, 270);
        assert_eq!(info.width, 1920, "文件里存的是没转过的尺寸");
        assert_eq!(info.display_size(), (1080, 1920));
        assert_eq!(info.channels, 1);
        assert_eq!(info.sample_rate, 44_100);
    }

    #[test]
    fn 纯音频素材没有帧率也没有画面() {
        let info = parse(AUDIO_ONLY).expect("能解析");
        assert!(!info.has_video());
        assert!(info.has_audio());
        assert_eq!(info.fps, None);
        assert_eq!(info.duration, Tick::from_secs(200.0));
        assert_eq!(info.channels, 2);
    }

    #[test]
    fn 没有时长或没有轨道时解析失败() {
        assert_eq!(parse("  Duration: N/A, bitrate: N/A\n"), None);
        assert_eq!(parse("No such file or directory\n"), None);
        assert_eq!(
            parse("  Duration: 00:00:01.00\n  Stream #0:0: Subtitle: subrip\n"),
            None,
            "只有字幕的文件时间线放不进去"
        );
    }

    #[test]
    fn 帧率取_fps_而不是_tbr() {
        let text = "  Duration: 00:00:01.00, start: 0\n  Stream #0:0: Video: h264, yuv420p, 1280x720, 30 fps, 29.97 tbr\n";
        assert_eq!(parse(text).unwrap().fps, Some(Rational::FPS_30));
    }

    // 需要本机 ffmpeg：`cargo test -- --ignored`。
    #[test]
    #[ignore = "需要本机 ffmpeg：cargo test -- --ignored"]
    fn 用真实文件探测时长帧率与分辨率() {
        let dir = std::env::temp_dir().join(format!("ngy-vp-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        let file = dir.join("clip.mp4");

        let executable = ffmpeg::executable().expect("本机没有 ffmpeg");
        let status = Command::new(&executable)
            .args([
                "-hide_banner",
                "-nostdin",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=2:size=320x240:rate=25",
                "-c:v",
                "mpeg4",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&file)
            .status()
            .expect("生成测试素材");
        assert!(status.success(), "ffmpeg 生成素材失败");

        let info = probe(&file).expect("探测测试素材");
        assert_eq!((info.width, info.height), (320, 240));
        assert_eq!(info.fps, Some(Rational::FPS_25));
        assert_eq!(info.pixel_format, "yuv420p");
        assert!(
            (info.duration.micros() - Tick::from_secs(2.0).micros()).abs() < 100_000,
            "时长应当在 2 秒左右：{}",
            info.duration.micros()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
