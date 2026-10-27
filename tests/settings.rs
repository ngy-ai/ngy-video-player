//! 首选项读写：置顶策略必须能读懂旧版本写下的配置，不能把整份设置作废。

use ngy_video_player::settings::{OnTop, Settings};

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
