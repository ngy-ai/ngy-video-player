//! 设置窗口：一个模态弹窗，左侧标签切换各组设置。
//!
//! 早期拆成「画面设置」和「视频渲染与解码」两个独立面板，文件关联则挂在菜单栏的
//! 「设置」菜单里：同一件事要在三处找，各自还记一份开合状态。这里把它们收进一个
//! 窗口——左侧标签选组，右侧只画当前这组。
//!
//! 布局照抄主流播放器：标题栏带关闭按钮，左列标签在底部放「恢复默认设置」。
//! 改动立刻生效并落盘，没有「确定/取消」——播放器调画面时看到的就是结果本身。

mod about;
mod associations;
mod panel;
mod picture;
mod playback;
mod reset;
mod shortcuts;
mod tabs;
mod video;

pub(in crate::app) use tabs::SettingsTab;
