//! 预览取文件的规则：素材比序列大、旁边有代理时用代理，导出不受影响。
//!
//! 规则本身在 `timeline::proxy`（纯函数，有单测）；这里只做两件事：读设置开关、
//! 把 [`crate::timeline::PreviewFile`] 摊成 `PathBuf` 给播放器用。

use std::path::Path;

use super::*;

impl PlayerApp {
    /// 预览要打开的文件。设置里关掉「预览用代理」时一律原片。
    ///
    /// 找不到素材时返回 `None`（调用方本来就要处理缺文件）。
    pub(crate) fn preview_path(&self, project: &Project, asset: AssetId) -> Option<PathBuf> {
        if !self.settings.preview_proxy {
            return project.asset_path(asset).map(Path::to_path_buf);
        }
        crate::timeline::preview_for(project, asset).map(|file| file.path().to_path_buf())
    }

    /// 这个素材在预览里是不是正在用代理。时间线上据此打「代理」标记。
    pub(crate) fn uses_proxy(&self, project: &Project, asset: AssetId) -> bool {
        self.settings.preview_proxy
            && crate::timeline::preview_for(project, asset).is_some_and(|file| file.is_proxy())
    }

    /// 把预览从这个文件上撤下来，交给重新生成代理的流程覆盖写。
    ///
    /// Windows 上文件被 mpv 读着的时候 ffmpeg 打不开它写入；所以主实例正在放它就先
    /// `Stop`，音频轨的实例一并卸掉。下一拍取帧会重新把（新的）文件加载回来。
    pub(crate) fn detach_preview(&mut self, path: &std::path::Path) {
        if self.timeline_preview.as_deref() == Some(path) {
            self.timeline_preview = None;
            self.send(Command::Stop);
        }
        self.silence_timeline_audio();
    }

    /// 「导入后自动为大素材补代理」。只记意图，真正的队列在导入完成时起。
    pub(crate) fn toggle_auto_proxy(&mut self) {
        self.settings.auto_proxy = !self.settings.auto_proxy;
        self.settings.save();
        if self.settings.auto_proxy {
            self.flash("已开启：导入后自动给比序列大的素材补代理");
        } else {
            self.flash("已关闭：导入后不再自动补代理，可以从素材库底栏手动发起");
        }
    }

    /// 设置窗口里的开关。下一次取帧 / 换片才生效，不做立刻重载：
    /// 播放中重载画面会黑一下，而用户改这一项时多半正要接着往下剪。
    pub(crate) fn toggle_preview_proxy(&mut self) {
        self.settings.preview_proxy = !self.settings.preview_proxy;
        self.settings.save();
        if self.settings.preview_proxy {
            self.flash("已开启：剪辑预览自动用代理（导出仍用原片）");
        } else {
            self.flash("已关闭：剪辑预览一律用原片");
        }
    }
}
