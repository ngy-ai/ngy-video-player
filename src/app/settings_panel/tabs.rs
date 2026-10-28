//! 设置窗口左侧的标签页。

use super::super::*;

/// 设置窗口里的分组。顺序就是左侧列表从上到下的顺序。
///
/// 分组的依据是「调什么」而不是「存在哪个字段」：画面那页改的是同一帧怎么摆，
/// 视频那页换的是谁来画、谁来解码，后者更像排障开关，混在一起只会把常用项挤远。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum SettingsTab {
    /// 比例、旋转、镜像。
    #[default]
    Picture,
    /// 渲染模式、解码方式、播放显卡。
    Video,
    /// 循环模式、置顶策略。
    Playback,
    /// 文件关联的注册与撤销。
    Associations,
    /// 键位表。
    Shortcuts,
    /// 版本、技术栈与运行环境。菜单栏「帮助 → 关于」直接停在这里。
    About,
}

impl SettingsTab {
    pub(in crate::app) const ALL: [SettingsTab; 6] = [
        SettingsTab::Picture,
        SettingsTab::Video,
        SettingsTab::Playback,
        SettingsTab::Associations,
        SettingsTab::Shortcuts,
        SettingsTab::About,
    ];

    pub(in crate::app) fn label(self) -> &'static str {
        match self {
            SettingsTab::Picture => "画面设置",
            SettingsTab::Video => "视频设置",
            SettingsTab::Playback => "播放设置",
            SettingsTab::Associations => "文件关联",
            SettingsTab::Shortcuts => "快捷键设置",
            SettingsTab::About => "关于",
        }
    }

    /// 标签上的小图标：一列纯文字在深色底上很难一眼扫出目标。
    pub(in crate::app) fn icon(self) -> IconName {
        match self {
            SettingsTab::Picture => IconName::Image,
            SettingsTab::Video => IconName::Monitor,
            SettingsTab::Playback => IconName::Clapperboard,
            SettingsTab::Associations => IconName::Link,
            SettingsTab::Shortcuts => IconName::Keyboard,
            SettingsTab::About => IconName::Info,
        }
    }
}
