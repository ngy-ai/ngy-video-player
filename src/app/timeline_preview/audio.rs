//! 音频轨预览：每条音频轨一路纯音频 mpv 实例，多条轨同时出声（M10-T05）。
//!
//! 主实例里装着画面那一路文件，mpv 一次只放一个文件的音频，也没法把「另一个文件的
//! 第 3 秒」混进来；所以音频轨另起纯音频实例，它们各放各的，由系统混音器加起来。
//! 导出侧用 `amix` 做同一件事，两边共用 [`preview::audio_layers`] 的选轨规则。
//!
//! 三条约定：
//!
//! * **实例按轨道惰性创建**：第一次真的撞上这条轨的片段时才起，没内容的时间线
//!   不为它养实例。上限 [`MAX_AUDIO_TRACKS`]，超出的轨先不响并提示一次。
//! * **音量 = 主音量 × 轨道增益**。mpv 的 `volume` 与导出的 `volume` 滤镜都是
//!   振幅倍数，两边电平因此对得上（差异只有 mpv 自己的重采样与限幅）。
//! * **只在换片段时下 `play`**：同一个文件每个节拍都 `seek` 会把声音打成断续的。

use super::*;
use crate::player::audio_preview::AudioPreview;
use crate::timeline::{Id, preview};

/// 同时出声的音频轨上限。与视频合成的取舍一致（ADR 0008）：多一条轨就多一份解码
/// 与一路音频输出，超出的轨先不响，界面上说清楚。
pub(crate) const MAX_AUDIO_TRACKS: usize = 4;

/// 一条音频轨的预览实例。
pub(crate) struct TrackAudio {
    pub(crate) track: Id,
    /// 实例里装着的文件；`None` 表示这一路当前没在放。
    pub(crate) file: Option<PathBuf>,
    /// 这条轨的音量（线性增益）。主音量一改就按它重算实例音量。
    pub(crate) gain: f32,
    pub(crate) player: AudioPreview,
}

impl PlayerApp {
    /// 让音频轨上的片段出声。`follow` 为真表示这是播放中的常规节拍。
    pub(crate) fn sync_timeline_audio(&mut self, at: Tick, playing: bool, follow: bool) {
        // 全局静音时音频轨也不该漏声，交给下面的「安静」分支处理。
        if self.settings.muted {
            self.silence_timeline_audio();
            return;
        }
        let Some(project) = self.project.as_ref() else {
            self.silence_timeline_audio();
            return;
        };
        // 先把这一拍要响的层取出来：素材路径、素材内时刻、轨道增益。
        let mut layers: Vec<(Id, PathBuf, f64, f32)> =
            preview::audio_layers(project.sequence(), at)
                .into_iter()
                .filter_map(|layer| {
                    let path = self.preview_path(project, layer.asset)?;
                    Some((layer.track, path, layer.source.secs(), layer.gain))
                })
                .collect();
        if layers.is_empty() {
            self.silence_timeline_audio();
            return;
        }
        // 超过上限只留靠上的几条（与「只叠最上面两路」同一个取舍），并且只说一次。
        if layers.len() > MAX_AUDIO_TRACKS {
            layers.truncate(MAX_AUDIO_TRACKS);
            if !self.audio_depth_warned {
                self.audio_depth_warned = true;
                self.flash(format!(
                    "预览一次最多同时放 {MAX_AUDIO_TRACKS} 条音频轨，靠下的轨暂时听不到"
                ));
            }
        } else {
            self.audio_depth_warned = false;
        }

        let volume = self.snapshot.volume;
        let speed = self.snapshot.speed;
        for (track, path, source, gain) in &layers {
            if !self.ensure_audio_slot(*track) {
                continue;
            }
            let Some(slot) = self
                .timeline_audio
                .iter_mut()
                .find(|slot| slot.track == *track)
            else {
                continue;
            };
            slot.gain = *gain;
            // mpv 的音量是百分比，上限 130；轨道增益乘在这个百分比上。
            slot.player
                .set_volume((volume * f64::from(*gain)).clamp(0.0, 130.0));
            slot.player.set_speed(speed);
            let switched = slot.file.as_deref() != Some(path.as_path());
            if follow && !switched {
                continue;
            }
            slot.file = Some(path.clone());
            slot.player.play(path, *source, playing);
        }
        // 这一套之外的轨按住：播放头走出了它的片段，或者它被静音 / 独奏挤掉了。
        for slot in &mut self.timeline_audio {
            if layers.iter().any(|(track, ..)| *track == slot.track) {
                continue;
            }
            if slot.file.take().is_some() {
                slot.player.pause();
            }
        }
        // 有音频轨在响：压住主画面自带的声音，否则原声和音乐各走各的。
        self.mute_main_for_audio(true);
    }

    /// 这条轨有没有实例；没有就起一个。返回是否可用。
    fn ensure_audio_slot(&mut self, track: Id) -> bool {
        if self.timeline_audio.iter().any(|slot| slot.track == track) {
            return true;
        }
        if self.timeline_audio_failed {
            return false;
        }
        let Some(engine) = self.engine.as_ref() else {
            return false;
        };
        let api = std::sync::Arc::clone(engine.api());
        match AudioPreview::spawn(api) {
            Ok(player) => {
                self.timeline_audio.push(TrackAudio {
                    track,
                    file: None,
                    gain: 1.0,
                    player,
                });
                true
            }
            Err(error) => {
                // 起不来就当没有这一路声音，主画面照旧；记住别再每拍重试刷日志。
                self.timeline_audio_failed = true;
                log_warn!("音频轨预览不可用: {error:#}");
                self.flash("音频轨预览不可用，预览里听不到音频轨");
                false
            }
        }
    }

    /// 主音量变了：每条实例按自己的轨道增益重算一次。
    ///
    /// 不这样做的话，拖音量条只影响画面那一路，音频轨还是老音量——两边一起放时
    /// 听起来像音量条坏了一半。
    pub(crate) fn apply_timeline_audio_volume(&mut self) {
        let volume = self.snapshot.volume;
        for slot in &mut self.timeline_audio {
            slot.player
                .set_volume((volume * f64::from(slot.gain)).clamp(0.0, 130.0));
        }
    }

    /// 预览变速 / 倒放：正向把倍速下发到每条实例，倒放只能按住它们。
    pub(crate) fn shuttle_timeline_audio(&mut self, rate: f64) {
        for slot in &mut self.timeline_audio {
            if rate < 0.0 {
                // 倒放没有声音（mpv 不能反向解码），与主画面同一条限制。
                slot.player.pause();
            } else {
                slot.player.set_speed(rate.abs());
            }
        }
    }

    /// 预览暂停：每条实例都按住，文件留着（播放头多半马上又走回来）。
    pub(crate) fn pause_timeline_audio(&mut self) {
        for slot in &mut self.timeline_audio {
            slot.player.pause();
        }
    }

    /// 这一刻没有音频片段：停住所有实例，把主画面的声音还回去。
    ///
    /// 重复调用是安全的（播放中每个节拍都会走一遍空档），这里先看有没有事可做，
    /// 免得每 33 毫秒往通道里塞一串 `Pause`。
    pub(crate) fn silence_timeline_audio(&mut self) {
        let idle = self.timeline_audio.iter().all(|slot| slot.file.is_none())
            && !self.timeline_audio_active;
        if idle {
            return;
        }
        for slot in &mut self.timeline_audio {
            // 清掉标记：下次再走进同一个片段时要重新对位，不能因为「文件没换」而跳过。
            if slot.file.take().is_some() {
                slot.player.pause();
            }
        }
        self.mute_main_for_audio(false);
    }

    /// 工程关掉 / 换工程：实例卸载文件，别在后台抱着上一个工程的素材。
    ///
    /// 实例本身留着（线程不便宜）：换工程后轨道 id 可能撞上，但 `file` 已经清空，
    /// 下一次 `play` 会按新路径重新加载。
    pub(crate) fn stop_timeline_audio(&mut self) {
        for slot in &mut self.timeline_audio {
            slot.player.stop();
            slot.file = None;
        }
        self.mute_main_for_audio(false);
    }

    /// 压住 / 还回主画面自带的声音。
    fn mute_main_for_audio(&mut self, mute: bool) {
        if self.timeline_audio_active == mute {
            return;
        }
        self.timeline_audio_active = mute;
        self.send(Command::SetMuted(mute || self.settings.muted));
    }
}
