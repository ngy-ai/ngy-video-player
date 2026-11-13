//! 遮罩（M10-T12）的成片验证：遮罩外的像素必须是黑的，里面还是原画面。
//!
//! 计划层的单测只能钉住「字符串里有没有 alphamerge」，钉不住 PGM 输入能不能被
//! ffmpeg 吃下、`alphamerge` 的输入顺序对不对、最后 alpha 是不是真的落到了黑底上。

use super::*;

/// 1 秒纯绿素材：像素好认，遮罩外应该变成黑。
fn make_green(dir: &Path, name: &str) -> PathBuf {
    let clip = dir.join(name);
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=green:size=320x240:rate=30:duration=1",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(&clip)
        .status()
        .expect("跑 ffmpeg 生成纯色素材");
    assert!(status.success(), "生成测试素材失败");
    clip
}

#[test]
#[ignore = "需要本机 ffmpeg"]
fn 遮罩外的像素变成黑() {
    let dir = scratch("export-mask");
    let clip = make_green(&dir, "green.mp4");

    let mut project = Project::new_with(SequenceSpec::new(320, 240, Rational::FPS_30));
    let asset = project.add_asset(&clip);
    let video = project.sequence_mut().tracks[0].id;
    put(&mut project, video, asset, 0.0, 1.0, 0.0);

    // 中间一半见方的硬边遮罩：0.25–0.75。
    project.sequence_mut().tracks[0].clips[0].mask = ngy_video_player::timeline::Mask {
        enabled: true,
        shape: ngy_video_player::timeline::MaskShape::Rect {
            x: 0.25,
            y: 0.25,
            width: 0.5,
            height: 0.5,
        },
        feather: 0.0,
        offset: (0.0, 0.0),
    };

    let output = run_export(&dir, &project, "mask-out.mp4");
    let inside = pixel_at(&output, 160, 120);
    let outside = pixel_at(&output, 10, 10);
    assert!(
        inside.1 > 120 && inside.0 < 80,
        "遮罩里还是绿的：{inside:?}"
    );
    assert!(
        outside.0 < 40 && outside.1 < 40 && outside.2 < 40,
        "遮罩外应当是黑的：{outside:?}"
    );

    // 边框上一行也是黑的：确认遮罩边界落在 0.25 而不是整幅。
    let corner = pixel_at(&output, 310, 230);
    assert!(corner.1 < 40, "右下角也在遮罩外：{corner:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 取成片某一帧某个像素的 RGB。`-ss 0` 取第一帧（遮罩在这里不随时间变）。
fn pixel_at(output: &Path, x: u32, y: u32) -> (u8, u8, u8) {
    let result = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(output)
        .args([
            // 先转成 rgb24 再裁：yuv420p 的 1×1 裁剪会被色度对齐拒掉。
            "-vf",
            &format!("format=rgb24,crop=1:1:{x}:{y}"),
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ])
        .output()
        .expect("解码一帧");
    let bytes = result.stdout;
    assert_eq!(bytes.len(), 3, "应当正好三个字节：{:?}", result.stderr);
    (bytes[0], bytes[1], bytes[2])
}
