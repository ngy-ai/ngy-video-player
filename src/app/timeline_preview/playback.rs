//! 预览播放：J/K/L 变速与倒放、播放时钟、拖动出声（scrub）与播放头步进。
//!
//! 播放头由墙钟推进，mpv 只负责「解出播放头那一刻」。倒放靠每拍补 seek 假装出来
//! （mpv 不能反向解码），代价是倒放没有声音、帧率也低。

use std::time::{Duration, Instant};

use super::*;
use crate::timeline::preview;

/// 时钟节拍：33 毫秒（30 Hz）。播放头是一根线，30 Hz 已经够顺；再快只是白重绘。
const TICK: Duration = Duration::from_millis(33);

/// 拖着播放头停手之后，声音还留多长。
///
/// 拖一次只该听见「指针下面那几声」：指针停住就安静，声音不能顺着序列一路放下去。
const SCRUB_TAIL: Duration = Duration::from_millis(150);

impl PlayerApp {
    /// `L`：正向播放，连按 1x → 2x → 4x（不循环，专业剪辑器的习惯）。
    pub(crate) fn shuttle_forward(&mut self, cx: &mut Context<Self>) {
        self.shuttle(1.0, cx);
    }

    /// `J`：倒放，连按 1x → 2x → 4x。倒放没有声音（见 `preview::rewind`）。
    pub(crate) fn shuttle_backward(&mut self, cx: &mut Context<Self>) {
        self.shuttle(-1.0, cx);
    }

    /// `K`：停下，并把档位清回原速（由 [`Self::stop_timeline_playback`] 负责）。
    ///
    /// 顺手记下「`K` 现在按着」：按住时 `J` / `L` 是逐帧。这里记账而不是
    /// `on_key_down`，是因为 `k` 绑了动作——绑了键位的按键到不了元素的
    /// `on_key_down` 监听（`on_key_up` 没有绑定，能到），松手就在那里清。
    pub(crate) fn shuttle_stop(&mut self, cx: &mut Context<Self>) {
        self.shuttle_key_held = true;
        if self.timeline_playing {
            self.stop_timeline_playback(cx);
        }
        cx.notify();
    }

    fn shuttle(&mut self, direction: f64, cx: &mut Context<Self>) {
        // 按住 `K` 再按 `J` / `L`：逐帧（`K` 已经把播放停住，这里只挪一帧）。
        // 与常见剪辑器一致，松手之前每按一下就一帧。
        if self.shuttle_key_held {
            self.step_timeline_frame(direction as i32, cx);
            return;
        }
        if !self.timeline_playing {
            // 没在播：J 从 -1x 起，L 从 1x 起。
            self.timeline_shuttle = direction;
            self.start_timeline_playback(cx);
            return;
        }
        let current = self.timeline_shuttle;
        self.timeline_shuttle = if current == 0.0 || current.signum() != direction {
            // 换了方向：回到 1x 重新起档，不然按一下就从 4x 倒放，容易失控。
            direction
        } else {
            direction * preview::shuttle_step(current.abs())
        };
        self.apply_shuttle();
        cx.notify();
    }

    /// 把档位落到播放上：正向交给 mpv 的 `speed`，反向按住 mpv 由时钟补 seek。
    fn apply_shuttle(&mut self) {
        let rate = self.timeline_shuttle;
        if self.timeline_preview.is_some() {
            self.send(Command::SetSpeed(rate.abs().max(0.1)));
            if rate < 0.0 {
                // 倒放时 mpv 不能自己往回走：按住它，画面每拍补一次 seek。
                self.send(Command::SetPaused(true));
            }
        }
        // 音频轨那几路是独立实例：倒放按住，正放跟上同一个倍速。
        self.shuttle_timeline_audio(rate);
        // 变速之后重新起算：不然上一段真实时间会按新倍速再乘一遍。
        self.timeline_last_tick = Instant::now();
        self.poke();
    }

    /// 播放 / 暂停预览。空格与标题条上的播放键共用这里。
    pub(crate) fn toggle_timeline_playback(&mut self, cx: &mut Context<Self>) {
        if self.timeline_playing {
            self.stop_timeline_playback(cx);
        } else {
            self.start_timeline_playback(cx);
        }
    }

    fn start_timeline_playback(&mut self, cx: &mut Context<Self>) {
        // 播放接管声音：拖动留下的 scrub 尾巴先收掉。
        self.stop_scrub_audio();
        let Some(project) = self.project.as_ref() else {
            // 起不来就别把档位留在那里：下一次按空格会带着上次的方向起跑。
            self.timeline_shuttle = 0.0;
            return;
        };
        if self.engine.is_none() {
            self.timeline_shuttle = 0.0;
            self.flash("播放引擎没有加载，无法预览");
            cx.notify();
            return;
        }
        let duration = project.sequence().duration();
        if duration <= Tick::ZERO {
            self.timeline_shuttle = 0.0;
            self.flash("时间线还是空的");
            cx.notify();
            return;
        }
        let range = project.ui.range();
        let stop = preview::stop_at(duration, range);
        let playhead = project.ui.playhead;
        // 倒放从播放头原地起步：停在结尾时按 `J` 是想从最后往回退，不是「从头
        // 再放一遍」——`start_at` 的那条「结尾回到入点」只对正向成立。
        let at = if self.timeline_shuttle < 0.0 {
            playhead.min(stop)
        } else {
            preview::start_at(playhead, stop, range)
        };
        if let Some(project) = self.project.as_mut() {
            project.ui.playhead = at;
        }

        self.timeline_playing = true;
        self.timeline_last_tick = Instant::now();
        if self.timeline_shuttle == 0.0 {
            self.timeline_shuttle = 1.0;
        }
        // 倒放时 mpv 只能被按住：`playing` 为假，画面靠时钟补 seek。
        let forward = self.timeline_shuttle > 0.0;
        // 先把画面挪到起点再放：从暂停处直接 PlayPause 会沿用 mpv 自己的位置，
        // 而它可能比播放头慢半拍（上一帧解码完到按下播放之间的那点时间）。
        self.preview_frame(at, forward);
        self.follow_playhead();
        self.apply_shuttle();
        self.start_timeline_clock(cx);
        self.poke();
        cx.notify();
    }

    /// 停下预览播放。播放头留在原地，画面停在当前这一帧。
    pub(crate) fn stop_timeline_playback(&mut self, cx: &mut Context<Self>) {
        if !self.timeline_playing {
            return;
        }
        self.timeline_playing = false;
        self.timeline_loaded_at = None;
        // 档位回到 1x：停一次再按播放必须是正常速度，不能接着上次的 4x 跑。
        self.timeline_shuttle = 0.0;
        // mpv 还在照原速往下走：不按住它，暂停之后声音还会继续播，画面也会
        // 跑出片段的出点。速率还给用户设的播放倍速，别把播放器那边的偏好改掉。
        if self.timeline_preview.is_some() {
            self.send(Command::SetPaused(true));
            self.send(Command::SetSpeed(self.settings.speed));
        }
        // 音频轨那几路是独立实例，主画面暂停管不到它们。
        self.pause_timeline_audio();
        // 合成器里那两路也一样：各自按自己的节奏放，主实例的暂停碰不到它们。
        if !self.composite_layers.is_empty() {
            let at = self
                .project
                .as_ref()
                .map(|project| project.ui.playhead)
                .unwrap_or(Tick::ZERO);
            self.present_composite(at, false, false);
        }
        cx.notify();
    }

    /// 时间线时钟：播放中按 [`TICK`] 的节拍推进播放头。
    ///
    /// 每次开始播放起一个新任务，靠 `timeline_clock` 的代数把它和下一次播放区分开：
    /// 暂停后立刻又按播放时，上一轮任务可能还没醒过来，只看 `timeline_playing`
    /// 会出现两个循环同时推进播放头（走两倍速）。
    fn start_timeline_clock(&mut self, cx: &mut Context<Self>) {
        self.timeline_clock = self.timeline_clock.wrapping_add(1);
        let generation = self.timeline_clock;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let alive = this.update(cx, |this, cx| {
                    if this.timeline_clock != generation || !this.timeline_playing {
                        return false;
                    }
                    this.tick_timeline_playback(cx);
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// 一个时钟节拍：推进播放头，让画面跟上。
    fn tick_timeline_playback(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            self.stop_timeline_playback(cx);
            return;
        };
        let has_range = project.ui.range().is_some();
        let stop = preview::stop_at(project.sequence().duration(), project.ui.range());
        let rate = self.timeline_shuttle;
        let elapsed = self.timeline_last_tick.elapsed();
        let (next, reached_end) = if rate < 0.0 {
            let start = project
                .ui
                .range()
                .map(|(start, _)| start)
                .unwrap_or(Tick::ZERO);
            let next = preview::rewind(project.ui.playhead, elapsed, -rate, start);
            (next, next <= start)
        } else {
            // 倍速取档位本身，不读 `snapshot.speed`：那是引擎回传的状态，改档之后
            // 要过几拍才追上。追上的那几拍里播放头按旧倍速走、合成层按新倍速放，
            // 实测 4x 时图层先跑到文件末尾，之后就再也 seek 不回去，画面永远停在
            // 最后一帧。档位是这里唯一可信的真相。
            let next = preview::advance(project.ui.playhead, elapsed, rate.abs(), stop);
            (next, next >= stop)
        };
        if let Some(project) = self.project.as_mut() {
            project.ui.playhead = next;
        }
        self.timeline_last_tick = Instant::now();

        if reached_end {
            let message = if rate < 0.0 {
                if has_range {
                    "退到工作区入点"
                } else {
                    "退到序列开头"
                }
            } else {
                "预览播放到结尾"
            };
            self.stop_timeline_playback(cx);
            self.flash(message);
        } else if rate < 0.0 {
            // 倒放：mpv 自己不会往回走，每一拍补一次 seek（与拖播放头同一条路径）。
            self.preview_frame(next, false);
        } else {
            self.preview_follow(next);
        }
        self.follow_playhead();
        cx.notify();
    }

    /// 拖播放头时让它出声（scrub）。
    ///
    /// 只出声、不动画面：拖的这段时间界面不再向 mpv 要新帧，它内部却照常解码，
    /// 于是听到的就是「指针下面那几声」。指针一停就按住，靠 [`SCRUB_TAIL`] 的
    /// 尾巴计时器——否则声音会顺着序列一路放下去，听着像误触了播放。
    ///
    /// 三处不出声：合成模式（主实例里什么都没装）、音频轨接管时（主实例被压成静音，
    /// 放开就是错的那一路）、全局静音。
    pub(crate) fn scrub_audio(&mut self, cx: &mut Context<Self>) {
        if self.timeline_playing
            || self.timeline_preview.is_none()
            || self.timeline_audio_active
            || self.settings.muted
        {
            return;
        }
        // 定格段本来就不该有声音：放开 mpv 会把它一路放出去。
        let frozen = match self.project.as_ref() {
            Some(project) => preview::resolve(project.sequence(), project.ui.playhead)
                .is_some_and(|frame| frame.frozen),
            None => return,
        };
        if frozen {
            return;
        }
        self.scrub_audio = true;
        self.send(Command::SetPaused(false));
        self.scrub_tail = self.scrub_tail.wrapping_add(1);
        let tail = self.scrub_tail;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SCRUB_TAIL).await;
            this.update(cx, |this, _| {
                if this.scrub_tail == tail {
                    this.stop_scrub_audio();
                }
            })
            .ok();
        })
        .detach();
    }

    /// 把 scrub 放开的声音按住。重复调用无副作用，播放中不碰暂停（那是时钟的事）。
    pub(crate) fn stop_scrub_audio(&mut self) {
        if !self.scrub_audio {
            return;
        }
        self.scrub_audio = false;
        if self.timeline_preview.is_some() && !self.timeline_playing {
            self.send(Command::SetPaused(true));
        }
    }

    /// 方向键在工程模式下挪播放头，而不是让 mpv 自己跳（它跳的是当前那一个素材）。
    pub(crate) fn seek_timeline_by(&mut self, delta: f64, cx: &mut Context<Self>) {
        self.stop_timeline_playback(cx);
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let duration = project.sequence().duration();
        let limit = crate::timeline::playhead_limit(&self.timeline, duration);
        let at = Tick::from_secs((project.ui.playhead.secs() + delta).clamp(0.0, limit));
        if let Some(project) = self.project.as_mut() {
            project.ui.playhead = at;
        }
        self.follow_playhead();
        self.preview_at_playhead(cx);
        let arrow = if delta > 0.0 { "⏩" } else { "⏪" };
        self.flash(format!("{arrow} {:.0}s", delta.abs()));
    }

    /// 逐帧步进：播放头按序列帧率走，落点吸附到帧（与切割、拖动用同一套帧网格）。
    pub(crate) fn step_timeline_frame(&mut self, steps: i32, cx: &mut Context<Self>) {
        self.stop_timeline_playback(cx);
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let fps = project.sequence().fps;
        let duration = project.sequence().duration();
        let limit = Tick::from_secs(crate::timeline::playhead_limit(&self.timeline, duration));
        let frame = project.ui.playhead.round_frames(fps) + i64::from(steps);
        let at = Tick::from_frames(frame, fps).clamp(Tick::ZERO, limit);
        if let Some(project) = self.project.as_mut() {
            project.ui.playhead = at;
        }
        self.follow_playhead();
        self.preview_at_playhead(cx);
        let arrow = if steps > 0 { "⏭" } else { "⏮" };
        self.flash(format!("{arrow} 逐帧"));
    }
}
