//! 首选项读写：置顶策略必须能读懂旧版本写下的配置，不能把整份设置作废。

use ngy_video_player::settings::{OnTop, Rotation, Settings, VideoAspect};

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
