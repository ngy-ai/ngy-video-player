//! 菜单栏两套结构的单测：只看标题集合，不需要 GPUI 窗口或 libmpv。

use super::*;

#[test]
fn playback_and_editing_menus_are_separate() {
    let playback = BarMenu::for_mode(false);
    let editing = BarMenu::for_mode(true);

    assert!(playback.contains(&BarMenu::Playback));
    assert!(playback.contains(&BarMenu::Subtitle));
    assert!(playback.contains(&BarMenu::Audio));
    assert!(!playback.contains(&BarMenu::Edit));
    assert!(!playback.contains(&BarMenu::Preview));

    assert!(editing.contains(&BarMenu::Edit));
    assert!(editing.contains(&BarMenu::Preview));
    assert!(!editing.contains(&BarMenu::Playback));
    assert!(!editing.contains(&BarMenu::Subtitle));
    assert!(!editing.contains(&BarMenu::Audio));
}

#[test]
fn both_modes_keep_shared_menus() {
    for menu in [
        BarMenu::File,
        BarMenu::View,
        BarMenu::Settings,
        BarMenu::Help,
    ] {
        assert!(BarMenu::for_mode(false).contains(&menu));
        assert!(BarMenu::for_mode(true).contains(&menu));
    }
}
