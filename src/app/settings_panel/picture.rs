//! 设置窗口 ·「画面」页：比例、旋转、镜像。
//!
//! 三组都是「同一帧怎么摆」，改完立刻作用在当前画面上，也跟着偏好保存。

use super::super::*;

impl PlayerApp {
    pub(super) fn picture_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let aspect = self.settings.video_aspect;
        let rotation = self.settings.video_rotate;

        // 两组都排成两列：六档比例/四档旋转竖着排会把右侧内容拉得很长。
        let aspect_rows: Vec<AnyElement> = VideoAspect::ALL
            .chunks(2)
            .enumerate()
            .map(|(row, pair)| {
                div()
                    .flex()
                    .gap(px(4.0))
                    .children(pair.iter().enumerate().map(|(column, mode)| {
                        picture_option(
                            format!("settings-aspect-{row}-{column}"),
                            mode.label(),
                            aspect == *mode,
                            false,
                            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                this.set_video_aspect(*mode);
                                cx.notify();
                            }),
                        )
                    }))
                    .into_any_element()
            })
            .collect();

        let rotation_rows: Vec<AnyElement> = Rotation::ALL
            .chunks(2)
            .enumerate()
            .map(|(row, pair)| {
                div()
                    .flex()
                    .gap(px(4.0))
                    .children(pair.iter().enumerate().map(|(column, angle)| {
                        picture_option(
                            format!("settings-rotate-{row}-{column}"),
                            angle.label(),
                            rotation == *angle,
                            false,
                            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                                this.set_rotation(*angle);
                                cx.notify();
                            }),
                        )
                    }))
                    .into_any_element()
            })
            .collect();

        let mirror_row = div()
            .flex()
            .gap(px(4.0))
            .child(picture_option(
                "settings-mirror-h",
                "水平镜像",
                self.settings.mirror_h,
                true,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.toggle_mirror(true);
                    cx.notify();
                }),
            ))
            .child(picture_option(
                "settings-mirror-v",
                "垂直镜像",
                self.settings.mirror_v,
                true,
                cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.toggle_mirror(false);
                    cx.notify();
                }),
            ))
            .into_any_element();

        div()
            .flex()
            .flex_col()
            .gap(px(18.0))
            .child(option_group("画面比例", aspect_rows))
            .child(option_group("画面旋转", rotation_rows))
            .child(option_group("画面镜像", vec![mirror_row]))
            .child(settings_note(
                "这几项立刻作用于当前画面，并且跟着偏好保存，换片和重启都保持。",
            ))
            .into_any_element()
    }
}
