//! 多轨合成预览（M10-T01）：画面交给 `player::compositor`。
//!
//! 只有「两条以上视频轨在播放头处都有画面」时才接管：单轨序列走原来的单路
//! 路径更省事，也不必为它多养两个 mpv 实例（ADR 0008 的决定 3）。

use super::*;
use crate::player::compositor;

impl PlayerApp {
    /// 让合成器接管这一帧。返回 `true` 表示画面由它负责，调用方别再驱动主实例。
    ///
    /// `force_seek` 为真（拖动、跳转、开始播放）时一定下发位置；播放中的常规节拍
    /// 只在「换了一层 / 播放状态变了 / 透明度变了」时下发——每 33 毫秒塞一次 seek
    /// 会让合成线程一直追着播放头跑。
    pub(crate) fn present_composite(&mut self, at: Tick, playing: bool, force_seek: bool) -> bool {
        if self.composite_failed {
            return false;
        }
        // 合成里那两路也是 mpv 在放：变速要一层层下发，否则它们按 1x 走，
        // 与按 2x / 4x 推进的播放头越离越远。倒放走「按住 + 逐拍 seek」，
        // 这里只给正数。
        let rate = if playing {
            self.timeline_shuttle.abs().max(0.1)
        } else {
            1.0
        };
        let Some(project) = self.project.as_ref() else {
            return false;
        };
        let layers = preview::layers(project.sequence(), at);
        if layers.len() < 2 {
            return false;
        }
        // 超过两路时只叠**最上面**两路：底下那几层在成片里也被压住了大半，
        // 少一层先看得见上面那层更重要。绝不静默：说一句再降级。
        let too_deep = layers.len() > compositor::MAX_LAYERS;
        let first = layers.len().saturating_sub(compositor::MAX_LAYERS);
        let mut specs = Vec::new();
        for layer in &layers[first..] {
            // 素材找不到的层没法解码，直接不叠（画面会缺一层，但不会整幅黑屏）。
            let Some(path) = self.preview_path(project, layer.frame.asset) else {
                continue;
            };
            specs.push(compositor::LayerSpec {
                path: path.to_path_buf(),
                source: layer.frame.source.secs(),
                playing: playing && !layer.frame.frozen,
                rate,
                opacity: layer.opacity,
            });
        }
        if specs.len() < 2 {
            return false;
        }
        if too_deep && !self.composite_depth_warned {
            self.composite_depth_warned = true;
            self.flash(format!(
                "预览一次只合成 {} 路画面，更下面的轨暂时看不到",
                compositor::MAX_LAYERS
            ));
        }

        if self.compositor.is_none() {
            let Some(engine) = self.engine.as_ref() else {
                return false;
            };
            let api = std::sync::Arc::clone(engine.api());
            match compositor::Compositor::spawn(api, engine.event_sender()) {
                Ok(player) => self.compositor = Some(player),
                Err(error) => {
                    // 不静默降级：说清楚退回单路的原因，并记住别再试。
                    self.composite_failed = true;
                    log_warn!("多轨合成预览不可用，退回单路: {error:#}");
                    self.flash("多轨合成预览不可用，已退回单路预览");
                    return false;
                }
            }
        }

        let same_count = self.composite_layers.len() == specs.len();
        let same_paths = same_count
            && self
                .composite_layers
                .iter()
                .zip(&specs)
                .all(|(old, new)| old.path == new.path);
        // 只是播放状态、速率或透明度变了：要更新，但绝不能顺手 seek 一下。
        let state_moved = !same_count
            || self.composite_layers.iter().zip(&specs).any(|(old, new)| {
                old.playing != new.playing
                    || (old.rate - new.rate).abs() > 0.001
                    || (old.opacity - new.opacity).abs() > 0.01
            });

        if (force_seek || !same_paths || state_moved)
            && let Some(compositor) = self.compositor.as_ref()
        {
            // 换了图层就得先对位；只改透明度 / 暂停时别动位置。
            compositor.present(specs.clone(), force_seek || !same_paths);
        }
        self.composite_layers = specs;
        true
    }

    /// 交回单路预览：合成器上不该再压着上一个工程的素材。
    pub(crate) fn leave_composite(&mut self) {
        if self.composite_layers.is_empty() {
            return;
        }
        if let Some(compositor) = self.compositor.as_ref() {
            compositor.stop();
        }
        self.composite_layers.clear();
        // 主实例在合成期间一直停着，文件可能已经不是它手里那个：让下一次刷新重新对位。
        self.timeline_preview = None;
    }
}
