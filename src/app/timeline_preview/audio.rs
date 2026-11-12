//! 音频轨预览：音频轨上的片段用独立的 mpv 实例出声。
//!
//! 主实例里装着画面那一路文件，mpv 一次只放一个文件的音频，也没法把「另一个文件的
//! 第 3 秒」混进来；所以音频轨另起纯音频实例，界面只负责在对的时机下命令。

use super::*;
use crate::timeline::preview;

impl PlayerApp {
    /// 让音频轨上的片段出声。
    ///
    /// 预览一次只放一路声音：音频轨上有内容时就把主画面自带的声音压住，否则两路叠在
    /// 一起（原声 + 音乐各走各的）比没有音乐更难受。走出音频片段再还回去。
    ///
    /// `follow` 为真表示这是播放中的常规节拍：只在换片段时动手——同一个文件每个节拍
    /// 都下命令，会让它一直重新 seek，听着就是不停打嗝。
    pub(super) fn sync_timeline_audio(&mut self, at: Tick, playing: bool, follow: bool) {
        let target = self.project.as_ref().and_then(|project| {
            let frame = preview::resolve_audio(project.sequence(), at)?;
            let path = project.asset_path(frame.asset)?.to_path_buf();
            Some((path, frame.source.secs()))
        });
        // 全局静音时音频轨也不该漏声，交给下面的「安静」分支处理。
        if self.settings.muted {
            self.silence_timeline_audio();
            return;
        }
        let Some((path, source)) = target else {
            self.silence_timeline_audio();
            return;
        };

        if self.timeline_audio.is_none() {
            let Some(engine) = self.engine.as_ref() else {
                return;
            };
            let api = std::sync::Arc::clone(engine.api());
            match crate::player::audio_preview::AudioPreview::spawn(api) {
                Ok(player) => {
                    player.set_volume(self.settings.volume);
                    player.set_speed(self.snapshot.speed);
                    self.timeline_audio = Some(player);
                }
                Err(error) => {
                    // 起不来就当没有这一路声音，主画面照旧。
                    log_warn!("音频轨预览不可用: {error:#}");
                    return;
                }
            }
        }

        let switched = self.timeline_audio_file.as_deref() != Some(path.as_path());
        if follow && !switched {
            return;
        }
        // 音频轨优先：压住主画面的自有声音。
        if !self.timeline_audio_active {
            self.timeline_audio_active = true;
            self.send(Command::SetMuted(true));
        }
        let Some(player) = self.timeline_audio.as_ref() else {
            return;
        };
        player.set_volume(self.settings.volume);
        player.set_speed(self.snapshot.speed);
        self.timeline_audio_file = Some(path.clone());
        player.play(&path, source, playing);
    }

    /// 这一刻没有音频片段：停住音频实例，把主画面的声音还回去。
    ///
    /// 重复调用是安全的（播放中每个节拍都会走一遍空档），这里先看有没有事可做，
    /// 免得每 33 毫秒往通道里塞一条 `Pause`。
    fn silence_timeline_audio(&mut self) {
        let idle = self.timeline_audio_file.is_none() && !self.timeline_audio_active;
        if idle {
            return;
        }
        if let Some(player) = self.timeline_audio.as_ref() {
            player.pause();
        }
        // 清掉标记：下次再走进同一个片段时要重新对位，不能因为「文件没换」而跳过。
        self.timeline_audio_file = None;
        if self.timeline_audio_active {
            self.timeline_audio_active = false;
            self.send(Command::SetMuted(self.settings.muted));
        }
    }

    /// 工程关掉 / 换工程：音频实例卸载文件，别在后台抱着上一个工程的素材。
    pub(crate) fn stop_timeline_audio(&mut self) {
        if let Some(player) = self.timeline_audio.as_ref() {
            player.stop();
        }
        self.timeline_audio_file = None;
        if self.timeline_audio_active {
            self.timeline_audio_active = false;
            self.send(Command::SetMuted(self.settings.muted));
        }
    }
}
