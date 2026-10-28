//! 画面与引擎选项：宽高比、旋转、镜像、渲染/解码后端。

use super::*;

impl PlayerApp {
    pub(super) fn set_video_aspect(&mut self, mode: VideoAspect) {
        if self.settings.video_aspect == mode {
            return;
        }
        self.settings.video_aspect = mode;
        self.settings.save();
        self.apply_picture();
    }

    pub(super) fn set_rotation(&mut self, rotation: Rotation) {
        if self.settings.video_rotate == rotation {
            return;
        }
        self.settings.video_rotate = rotation;
        self.settings.save();
        // 旋转不动 mpv 的属性，只改取帧时的那次转置；渲染目标一变就会重画一帧。
        self.apply_picture();
    }

    pub(super) fn toggle_mirror(&mut self, horizontal: bool) {
        if horizontal {
            self.settings.mirror_h = !self.settings.mirror_h;
        } else {
            self.settings.mirror_v = !self.settings.mirror_v;
        }
        self.settings.save();
        self.apply_picture();
    }

    /// 把画面设置整体推给 mpv。
    ///
    /// 每次都全量下发而不是算差量：这几个属性都很便宜，而「算差量」要求界面
    /// 精确记住 mpv 那侧的现状，一旦有一次命令失败就会长期错位。
    ///
    /// 旋转**不在这里**——libmpv 的软件渲染路径不认 `video-rotate`，
    /// 那一圈是在取帧时自己转的（见 `player::frame`）。
    pub(super) fn apply_picture(&self) {
        let aspect = self.settings.video_aspect;
        self.send(Command::SetOption {
            name: "video-aspect-override".into(),
            value: aspect.override_value().unwrap_or("no").to_string(),
        });
        // 拉伸靠关掉等比缩放；裁剪靠 panscan 把放大的那部分裁掉，两者互斥。
        self.send(Command::SetOption {
            name: "keepaspect".into(),
            value: if aspect.stretches() { "no" } else { "yes" }.to_string(),
        });
        self.send(Command::SetOption {
            name: "panscan".into(),
            value: if aspect.crops() { "1.0" } else { "0.0" }.to_string(),
        });
        // 镜像在 mpv 里是滤镜链，不是开关属性：直接写整条 `vf` 才是幂等的，
        // 用 `hflip` 命令的话连点两次会互相抵消。空串就是「一条滤镜都不挂」。
        let mut filters = Vec::new();
        if self.settings.mirror_h {
            filters.push("hflip");
        }
        if self.settings.mirror_v {
            filters.push("vflip");
        }
        self.send(Command::SetOption {
            name: "vf".into(),
            value: filters.join(","),
        });
        // 旋转由我们自己算，显式清零，防止用户自己的 mpv 配置把它又打开一次。
        self.send(Command::SetOption {
            name: "video-rotate".into(),
            value: "0".to_string(),
        });
        // 上面这些都不改渲染目标的尺寸：暂停时 mpv 不会送新帧，
        // 不主动点一下，面板里选完东西画面纹丝不动。
        if let Some(engine) = &self.engine {
            engine.request_redraw();
        }
    }

    // -- 视频渲染与解码 ----------------------------------------------------

    /// 渲染模式只影响「按多大尺寸渲染」：改动落在渲染目标上，下一次布局就生效。
    pub(super) fn set_render_mode(&mut self, mode: RenderMode) {
        if self.settings.render_mode == mode {
            return;
        }
        self.settings.render_mode = mode;
        self.settings.save();
        // 暂停时渲染目标变了也不会立刻出新帧，主动点一下重绘。
        if let Some(engine) = &self.engine {
            engine.request_redraw();
        }
        self.flash(format!("渲染模式：{}", mode.label()));
    }

    /// 解码方式要交给 mpv 的 `hwdec`：mpv 会自己重建解码链，不用重开文件。
    pub(super) fn set_decode_mode(&mut self, mode: DecodeMode) {
        if self.settings.decode == mode {
            return;
        }
        self.settings.decode = mode;
        self.settings.save();
        self.send(Command::SetOption {
            name: "hwdec".into(),
            value: mode.hwdec().to_string(),
        });
        self.flash(format!("解码方式：{}", mode.label()));
    }

    /// 播放显卡：`None` 是智能选择，`Some` 是指定显卡名。
    pub(super) fn set_gpu(&mut self, gpu: Option<String>) {
        if self.settings.gpu == gpu {
            return;
        }
        let label = gpu.clone().unwrap_or_else(|| "智能选择".to_string());
        self.settings.gpu = gpu.clone();
        self.settings.save();
        // 只有 Windows 的 D3D11 后端认这个选项；别的平台照常记住选择，
        // 不要为了一件没有对应实现的事去发一条注定失败的 mpv 命令。
        if cfg!(windows) {
            self.send(Command::SetOption {
                name: "d3d11-adapter".into(),
                value: gpu.unwrap_or_else(|| "auto".to_string()),
            });
        }
        self.flash(format!("播放显卡：{label}"));
    }

    /// 配置里选中的那块显卡是否还在本机列表里。
    ///
    /// 换了机器、拔了外置显卡之后配置里会留下一个对不上的名字：这时按「智能选择」
    /// 处理，但不去改写配置——那块卡插回来的时候选择也就跟着回来了。
    pub(super) fn gpu_selected(&self) -> Option<&str> {
        self.settings
            .gpu
            .as_deref()
            .filter(|name| self.gpu_adapters.iter().any(|adapter| adapter == name))
    }

    /// 「智能选择」括号里写的当前显卡：没指定（或指定的那块不在了）时
    /// 按系统枚举到的第一块算。
    pub(super) fn current_gpu_label(&self) -> String {
        match self.gpu_selected() {
            Some(name) => name.to_string(),
            None => self
                .gpu_adapters
                .first()
                .cloned()
                .unwrap_or_else(|| "系统默认显卡".to_string()),
        }
    }
}
