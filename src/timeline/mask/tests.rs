use super::*;

#[test]
fn 矩形遮罩里外分明() {
    let mask = Mask {
        enabled: true,
        shape: MaskShape::Rect {
            x: 0.25,
            y: 0.25,
            width: 0.5,
            height: 0.5,
        },
        feather: 0.0,
        offset: (0.0, 0.0),
    };
    // 100×100 的画面：矩形在 25..75，中心不透明、四角全透明。
    assert_eq!(mask.alpha_at(50.0, 50.0, 100, 100), 1.0);
    assert_eq!(mask.alpha_at(5.0, 5.0, 100, 100), 0.0);
    assert_eq!(mask.alpha_at(95.0, 50.0, 100, 100), 0.0);
    assert_eq!(mask.alpha_at(50.0, 95.0, 100, 100), 0.0);

    let plane = mask.alpha_plane(100, 100);
    assert_eq!(plane.len(), 10_000);
    assert_eq!(plane[50 * 100 + 50], 255, "中心不透明");
    assert_eq!(plane[0], 0, "左上角透明");
}

#[test]
fn 羽化在边界上取一半() {
    let mask = Mask {
        enabled: true,
        shape: MaskShape::Rect {
            x: 0.2,
            y: 0.2,
            width: 0.6,
            height: 0.6,
        },
        // 相对短边 0.2，100 像素就是 20 像素的过渡带。
        feather: 0.2,
        offset: (0.0, 0.0),
    };
    // 边界在 x = 20：这一点正好一半。
    assert!((mask.alpha_at(20.0, 50.0, 100, 100) - 0.5).abs() < 0.02);
    // 往外走半个过渡带到 0，往里走半个到 1。
    assert!(mask.alpha_at(10.0, 50.0, 100, 100) < 0.01);
    assert!(mask.alpha_at(30.0, 50.0, 100, 100) > 0.99);
    // 越往中心越实：中间肯定是 1。
    assert_eq!(mask.alpha_at(50.0, 50.0, 100, 100), 1.0);
}

#[test]
fn 位移把遮罩整体挪开() {
    let base = Mask {
        enabled: true,
        shape: MaskShape::Ellipse {
            x: 0.4,
            y: 0.4,
            width: 0.2,
            height: 0.2,
        },
        feather: 0.0,
        offset: (0.0, 0.0),
    };
    assert_eq!(base.alpha_at(50.0, 50.0, 100, 100), 1.0);

    // 往右挪 30%：原来的中心变成透明，30 像素右边才是实心。
    let moved = Mask {
        offset: (0.3, 0.0),
        ..base.clone()
    };
    assert_eq!(moved.alpha_at(50.0, 50.0, 100, 100), 0.0);
    assert_eq!(moved.alpha_at(80.0, 50.0, 100, 100), 1.0);
}

#[test]
fn 路径遮罩按多边形判断内外() {
    let mask = Mask {
        enabled: true,
        shape: MaskShape::Path(vec![(0.2, 0.2), (0.8, 0.2), (0.5, 0.8)]),
        feather: 0.0,
        offset: (0.0, 0.0),
    };
    assert_eq!(mask.alpha_at(50.0, 35.0, 100, 100), 1.0, "三角形里的点");
    assert_eq!(mask.alpha_at(20.0, 70.0, 100, 100), 0.0, "三角形外的点");

    // 退化路径整幅透明，而不是算出一片实心。
    let degenerate = Mask {
        shape: MaskShape::Path(vec![(0.2, 0.2)]),
        ..mask
    };
    assert_eq!(degenerate.alpha_at(50.0, 35.0, 100, 100), 0.0);
}

#[test]
fn 写出的_pgm_头与像素数对得上() {
    let mask = Mask {
        enabled: true,
        ..Mask::default()
    };
    let bytes = mask.pgm(16, 8);
    let header = b"P5\n16 8\n255\n";
    assert!(bytes.starts_with(header));
    assert_eq!(bytes.len(), header.len() + 16 * 8, "灰度一像素一字节");
}

#[test]
fn 相对坐标换到不同分辨率构图一致() {
    let mask = Mask {
        enabled: true,
        shape: MaskShape::Rect {
            x: 0.25,
            y: 0.5,
            width: 0.5,
            height: 0.25,
        },
        ..Mask::default()
    };
    // 1080p 与 4K 各自按比例落点：中心位置的比例相同。
    let (x1, y1, w1, h1) = mask.shape.pixel_bounds(1920, 1080);
    let (x2, y2, w2, h2) = mask.shape.pixel_bounds(3840, 2160);
    assert_eq!((x1, y1, w1, h1), (480, 540, 960, 270));
    assert_eq!((x2, y2, w2, h2), (960, 1080, 1920, 540));
}

#[test]
fn 像素外接矩形是偶数且不出界() {
    let shape = MaskShape::Rect {
        x: 0.9,
        y: 0.9,
        width: 0.5,
        height: 0.5,
    };
    let (x, y, w, h) = shape.pixel_bounds(1919, 1079);
    assert!(x < 1919 && y < 1079);
    assert!(w % 2 == 0 && h % 2 == 0, "{w}x{h}");
    assert!(x + w <= 1919, "{}", x + w);
    assert!(y + h <= 1079, "{}", y + h);
}

#[test]
fn 路径按外接矩形() {
    let shape = MaskShape::Path(vec![(0.2, 0.1), (0.8, 0.3), (0.5, 0.9)]);
    let (x, y, w, h) = shape.pixel_bounds(1000, 1000);
    assert_eq!((x, y), (200, 100));
    assert_eq!((w, h), (600, 800));
}

#[test]
fn 退化形状不算生效() {
    let mut mask = Mask::default();
    assert!(!mask.effective(), "默认没启用");
    mask.enabled = true;
    assert!(mask.effective(), "矩形启用即生效");
    mask.shape = MaskShape::Path(vec![(0.1, 0.1), (0.2, 0.2)]);
    assert!(!mask.effective(), "两个点的路径画不出面");
    mask.shape = MaskShape::Path(vec![(0.1, 0.1), (0.2, 0.2), (0.3, 0.1)]);
    assert!(mask.effective());
}

#[test]
fn 羽化与位移夹进范围() {
    let mut mask = Mask {
        feather: 9.0,
        offset: (3.0, -9.0),
        ..Mask::default()
    };
    assert_eq!(mask.normalized_feather(), MAX_FEATHER);
    mask.feather = f32::NAN;
    assert_eq!(mask.normalized_feather(), 0.0);
    assert_eq!(mask.offset_pixels(1920, 1080), (1920, -1080));
}

#[test]
fn 坐标坏值夹回画面内() {
    let shape = MaskShape::Rect {
        x: -1.0,
        y: 5.0,
        width: 9.0,
        height: f32::NAN,
    }
    .normalized();
    assert_eq!(
        shape,
        MaskShape::Rect {
            x: 0.0,
            y: 1.0,
            width: 1.0,
            height: 0.0
        }
    );
}

#[test]
fn 遮罩能存进工程() {
    let mut project = crate::timeline::Project::default();
    let id = project.add_asset(std::path::Path::new("C:/素材/a.mp4"));
    let path = std::env::temp_dir().join("ngy-mask.ngyproj");
    // 遮罩挂在片段上：借一条临时序列走一遍序列化。
    let mut sequence =
        crate::timeline::Sequence::new("序列", crate::timeline::Rational::FPS_25, 1920, 1080);
    sequence.tracks.clear();
    let track_id = sequence.add_track(crate::timeline::TrackKind::Video);
    let clip_id = sequence.alloc_id();
    let mut clip = crate::timeline::ClipItem::new(
        clip_id,
        id,
        crate::timeline::Tick::ZERO,
        crate::timeline::Tick::from_secs(1.0),
        crate::timeline::Tick::ZERO,
    );
    clip.mask = Mask {
        enabled: true,
        shape: MaskShape::Ellipse {
            x: 0.2,
            y: 0.2,
            width: 0.6,
            height: 0.6,
        },
        feather: 0.1,
        offset: (0.05, 0.0),
    };
    if let Some(track) = sequence.track_mut(track_id) {
        track.clips.push(clip);
    }
    let sequence_id = sequence.id;
    project.sequences = vec![sequence];
    project.active_sequence = sequence_id;
    project.save(&path).expect("保存");
    let opened = crate::timeline::Project::open(&path).expect("打开");
    let back = opened.sequence().clip(clip_id).expect("片段还在");
    assert!(back.mask.enabled);
    assert_eq!(back.mask.feather, 0.1);
    assert_eq!(back.mask.shape.label(), "椭圆");
    let _ = std::fs::remove_file(&path);
}
