//! 首选项读写：置顶策略必须能读懂旧版本写下的配置，不能把整份设置作废。

use ngy_video_player::settings::{DecodeMode, OnTop, RenderMode, Rotation, Settings, VideoAspect};

#[test]
fn 置顶策略能存能读() {
    for mode in OnTop::ALL {
        let text = serde_json::to_string(&mode).unwrap();
        let back: OnTop = serde_json::from_str(&text).unwrap();
        assert_eq!(back, mode, "存的是 {text}");
    }
}

#[test]
fn 旧的布尔置顶键仍然认() {
    let on: Settings = serde_json::from_str(r#"{"always_on_top": true}"#).unwrap();
    assert_eq!(on.on_top, OnTop::Always);

    let off: Settings = serde_json::from_str(r#"{"always_on_top": false}"#).unwrap();
    assert_eq!(off.on_top, OnTop::Never);
}

#[test]
fn 缺少置顶项时用默认值且不影响其它字段() {
    let settings: Settings = serde_json::from_str(r#"{"volume": 50.0}"#).unwrap();
    assert_eq!(settings.on_top, OnTop::Never);
    assert_eq!(settings.volume, 50.0);
}

#[test]
fn 画面设置能存能读() {
    for aspect in VideoAspect::ALL {
        let text = serde_json::to_string(&aspect).unwrap();
        let back: VideoAspect = serde_json::from_str(&text).unwrap();
        assert_eq!(back, aspect, "存的是 {text}");
    }
    for rotation in Rotation::ALL {
        let text = serde_json::to_string(&rotation).unwrap();
        let back: Rotation = serde_json::from_str(&text).unwrap();
        assert_eq!(back, rotation, "存的是 {text}");
    }
}

#[test]
fn 旧配置里没有画面设置时用默认值() {
    let settings: Settings = serde_json::from_str(r#"{"volume": 50.0}"#).unwrap();
    assert_eq!(settings.video_aspect, VideoAspect::Source);
    assert_eq!(settings.video_rotate, Rotation::None);
    assert!(!settings.mirror_h);
    assert!(!settings.mirror_v);
}

#[test]
fn 认不出的画面设置退回默认值而不是整份作废() {
    // 手改配置写错比例名、或者别的版本写进一个怪角度，都不该让用户丢掉音量等设置。
    let settings: Settings =
        serde_json::from_str(r#"{"video_aspect": "5:4", "video_rotate": 45, "volume": 42.0}"#)
            .unwrap();
    assert_eq!(settings.video_aspect, VideoAspect::Source);
    assert_eq!(settings.video_rotate, Rotation::None);
    assert_eq!(settings.volume, 42.0);
}

#[test]
fn 渲染与解码设置能存能读() {
    for mode in RenderMode::ALL {
        let text = serde_json::to_string(&mode).unwrap();
        let back: RenderMode = serde_json::from_str(&text).unwrap();
        assert_eq!(back, mode, "存的是 {text}");
    }
    for mode in DecodeMode::ALL {
        let text = serde_json::to_string(&mode).unwrap();
        let back: DecodeMode = serde_json::from_str(&text).unwrap();
        assert_eq!(back, mode, "存的是 {text}");
    }
}

#[test]
fn 旧配置里的_hwdec_字符串仍能读成解码模式() {
    // 老版本直接存 mpv 的取值；换字段名之后这些文件不能整份失效。
    for (raw, expected) in [
        ("auto-safe", DecodeMode::Smart),
        ("auto-copy", DecodeMode::Hardware),
        ("no", DecodeMode::Software),
    ] {
        let text = format!(r#"{{"hwdec": "{raw}", "volume": 42.0}}"#);
        let settings: Settings = serde_json::from_str(&text).unwrap();
        assert_eq!(settings.decode, expected, "读到的是 {raw}");
        assert_eq!(settings.volume, 42.0);
    }
}

#[test]
fn 认不出的解码方式退回软件解码() {
    let settings: Settings = serde_json::from_str(r#"{"hwdec": "cuda"}"#).unwrap();
    assert_eq!(settings.decode, DecodeMode::Software);
}

#[test]
fn 缺省时用性能模式与智能选择显卡() {
    let settings: Settings = serde_json::from_str(r#"{"volume": 50.0}"#).unwrap();
    assert_eq!(settings.render_mode, RenderMode::Performance);
    assert_eq!(settings.decode, DecodeMode::Software);
    assert_eq!(settings.gpu, None);
    // 性能模式封顶渲染高度，稳定模式不封顶：这是两档唯一的行为差别。
    assert_eq!(RenderMode::Performance.height_cap(), Some(720));
    assert_eq!(RenderMode::Stable.height_cap(), None);
}

#[test]
fn 指定的显卡能存能读() {
    let settings: Settings = serde_json::from_str(r#"{"gpu": "NVIDIA GeForce RTX 3080"}"#).unwrap();
    assert_eq!(settings.gpu.as_deref(), Some("NVIDIA GeForce RTX 3080"));
}
