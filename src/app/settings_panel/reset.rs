//! 「恢复默认设置」：把设置窗口里能看到的选项拉回出厂值。

use super::super::*;

impl PlayerApp {
    /// 只重置设置窗口里出现的项。
    ///
    /// 音量、播放进度、窗口大小、面板宽度都是「用着用着调出来的」，恢复出厂值会让
    /// 用户为了一次误点重新调一遍；它们也不在这扇窗口里，不该被一个看不见的按钮改掉。
    pub(super) fn restore_default_settings(&mut self, cx: &mut Context<Self>) {
        let defaults = Settings::default();
        self.settings.video_aspect = defaults.video_aspect;
        self.settings.video_rotate = defaults.video_rotate;
        self.settings.mirror_h = defaults.mirror_h;
        self.settings.mirror_v = defaults.mirror_v;
        self.settings.render_mode = defaults.render_mode;
        self.settings.decode = defaults.decode;
        self.settings.gpu = defaults.gpu.clone();
        self.settings.loop_mode = defaults.loop_mode;
        self.settings.on_top = defaults.on_top;
        self.settings.maintain_associations = defaults.maintain_associations;
        self.settings.save();

        // 各项的生效路径不一样，这里整体重放一遍：setter 都带「值没变就不做事」的
        // 短路，重置之后它们的入参恰好和刚写进去的值相同，靠 setter 什么都不会发生。
        self.apply_picture();
        self.send(Command::SetOption {
            name: "hwdec".into(),
            value: self.settings.decode.hwdec().to_string(),
        });
        // 只有 Windows 的 D3D11 后端认显卡选项，别的平台不必发一条注定失败的命令。
        if cfg!(windows) {
            self.send(Command::SetOption {
                name: "d3d11-adapter".into(),
                value: self
                    .settings
                    .gpu
                    .clone()
                    .unwrap_or_else(|| "auto".to_string()),
            });
        }
        if let Some(engine) = &self.engine {
            engine.request_redraw();
        }
        // 置顶不用在这里动手：下一帧的 `sync_on_top` 会比对期望值和已下发值。
        self.flash("已恢复默认设置");
        cx.notify();
    }
}
