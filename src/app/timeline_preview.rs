//! 工程模式预览：播放头落在哪段素材上，就把那一刻的画面送进播放器。
//!
//! 取帧规则在 `timeline::preview`（纯函数，和导出共用一份选轨逻辑）。这里只做
//! 「怎么喂给 mpv」这一件事，外加播放时推进播放头的时钟。
//!
//! 同一个文件只 seek，换文件才 `Load`：换片有几百毫秒的加载，拖播放头时每挪几个
//! 像素就换一次片，画面会一顿一顿地黑。
//!
//! 播放不是让 mpv 自己从头放到尾：播放头由墙钟推进（见
//! [`PlayerApp::start_timeline_clock`]），mpv 只负责解出播放头那一刻的画面。
//! 走出片段就重新加载，偏差超过阈值才补一次 seek——每帧都 seek 会让声音一直打嗝。

use std::path::Path;
use std::time::{Duration, Instant};

use super::*;
use crate::timeline::preview;

/// 时钟节拍：33 毫秒（30 Hz）。播放头是一根线，30 Hz 已经够顺；再快只是白重绘。
const TICK: Duration = Duration::from_millis(33);

/// 加载 / seek 之后的宽限期：这段时间里 mpv 的位置还没追上，不能拿来判断偏差。
///
/// 不宜太长：`Load` 带的 seek 只落到关键帧上，长 GOP 的素材可能从入点前一两秒
/// 开始放，宽限期一过就要靠偏差纠正补一次精确 seek，那之前看到的画面偏早。
const LOAD_GRACE: Duration = Duration::from_millis(300);

impl PlayerApp {
    /// 按当前播放头刷新预览画面（拖动、跳转之后）。
    pub(super) fn preview_at_playhead(&mut self, cx: &mut Context<Self>) {
        // 时间线一动，试听就结束：预览区的主语回到序列，而不是双击过的那个原片。
        self.audition = None;
        let Some(at) = self.project.as_ref().map(|project| project.ui.playhead) else {
            return;
        };
        self.preview_frame(at, false);
        self.poke();
        cx.notify();
    }

    /// 把画面挪到序列时刻 `at`。`playing` 为真时让 mpv 从那里接着放。
    ///
    /// 返回这一刻有没有画面：空档、素材缺失时是 `false`。没有画面时什么命令都不发，
    /// 让 mpv 留着上一帧——突然黑屏比停在上一帧更像「这里什么都没有」。
    fn preview_frame(&mut self, at: Tick, playing: bool) -> bool {
        let Some(project) = self.project.as_ref() else {
            return false;
        };
        let Some(frame) = preview::resolve(&project.sequence, at) else {
            return false;
        };
        let Some(path) = project.asset_path(frame.asset).map(Path::to_path_buf) else {
            return false;
        };
        let source = frame.source.secs();

        if self.timeline_preview.as_deref() != Some(path.as_path()) {
            self.timeline_preview = Some(path.clone());
            self.timeline_loaded_at = Some(Instant::now());
            self.begin_load(&path);
            self.send(Command::Load {
                url: path.to_string_lossy().into_owned(),
                start: Some(source),
            });
        } else {
            self.send(Command::SeekAbsolute(source));
            // 重新计时：seek 之后 mpv 的位置要过一会儿才追上，那之前别拿它做判断。
            self.timeline_loaded_at = Some(Instant::now());
        }
        // 拖播放头要看的是「这一帧」，不是从这儿接着放；播放时则要从这里接着走。
        self.send(Command::SetPaused(!playing));
        true
    }

    /// 播放 / 暂停预览。空格与标题条上的播放键共用这里。
    pub(super) fn toggle_timeline_playback(&mut self, cx: &mut Context<Self>) {
        if self.timeline_playing {
            self.stop_timeline_playback(cx);
        } else {
            self.start_timeline_playback(cx);
        }
    }

    fn start_timeline_playback(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        if self.engine.is_none() {
            self.flash("播放引擎没有加载，无法预览");
            cx.notify();
            return;
        }
        let duration = project.sequence.duration();
        if duration <= Tick::ZERO {
            self.flash("时间线还是空的");
            cx.notify();
            return;
        }
        let range = project.ui.range();
        let stop = preview::stop_at(duration, range);
        let at = preview::start_at(project.ui.playhead, stop, range);
        if let Some(project) = self.project.as_mut() {
            project.ui.playhead = at;
        }

        self.timeline_playing = true;
        self.timeline_last_tick = Instant::now();
        // 先把画面挪到起点再放：从暂停处直接 PlayPause 会沿用 mpv 自己的位置，
        // 而它可能比播放头慢半拍（上一帧解码完到按下播放之间的那点时间）。
        self.preview_frame(at, true);
        self.follow_playhead();
        self.start_timeline_clock(cx);
        self.poke();
        cx.notify();
    }

    /// 停下预览播放。播放头留在原地，画面停在当前这一帧。
    pub(super) fn stop_timeline_playback(&mut self, cx: &mut Context<Self>) {
        if !self.timeline_playing {
            return;
        }
        self.timeline_playing = false;
        self.timeline_loaded_at = None;
        // mpv 还在照原速往下走：不按住它，暂停之后声音还会继续播，画面也会
        // 跑出片段的出点。
        if self.timeline_preview.is_some() {
            self.send(Command::SetPaused(true));
        }
        cx.notify();
    }

    /// 时间线时钟：播放中按 [`TICK`] 的节拍推进播放头。
    ///
    /// 每次开始播放起一个新任务，靠 `timeline_clock` 的代数把它和下一次播放区分开：
    /// 暂停后立刻又按播放时，上一轮任务可能还没醒过来，只看 `timeline_playing`
    /// 会出现两个循环同时推进播放头（走两倍速）。
    pub(super) fn start_timeline_clock(&mut self, cx: &mut Context<Self>) {
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
        let stop = preview::stop_at(project.sequence.duration(), project.ui.range());
        // 倍速是 mpv 的播放速度：时钟必须乘同一个数，否则改了倍速之后音画和
        // 播放头会越离越远。
        let speed = self.snapshot.speed;
        let elapsed = self.timeline_last_tick.elapsed();
        let next = preview::advance(project.ui.playhead, elapsed, speed, stop);
        let reached_end = next >= stop;
        if let Some(project) = self.project.as_mut() {
            project.ui.playhead = next;
        }
        self.timeline_last_tick = Instant::now();

        if reached_end {
            self.stop_timeline_playback(cx);
            self.flash("预览播放到结尾");
        } else {
            self.preview_follow(next);
        }
        self.follow_playhead();
        cx.notify();
    }

    /// 播放中的画面跟随：只在换素材、走出空档、偏差过大时才给 mpv 下命令。
    ///
    /// 每个节拍都 seek 一次是最省事的写法，但 mpv 每次精确 seek 都要重新解码，
    /// 声音会一直打嗝。正常播放时它自己就在正确的速度上走，没有必要纠正。
    fn preview_follow(&mut self, at: Tick) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let Some(frame) = preview::resolve(&project.sequence, at) else {
            // 空档：这一刻没有任何画面。停住 mpv，播放头照常往前走。
            if !self.snapshot.paused {
                self.send(Command::SetPaused(true));
            }
            return;
        };
        let Some(path) = project.asset_path(frame.asset).map(Path::to_path_buf) else {
            return;
        };
        let source = frame.source.secs();

        if self.timeline_preview.as_deref() != Some(path.as_path()) {
            self.timeline_preview = Some(path.clone());
            self.timeline_loaded_at = Some(Instant::now());
            self.begin_load(&path);
            self.send(Command::Load {
                url: path.to_string_lossy().into_owned(),
                start: Some(source),
            });
            self.send(Command::SetPaused(false));
            return;
        }
        // 上一段空档里把它停住了（或者刚按下播放）：放行。
        if self.snapshot.paused {
            self.send(Command::SetPaused(false));
        }
        // mpv 还停在别的文件上，或者刚加载完还没追上：这时候的位置不能当真相。
        if self.snapshot.path.as_deref() != self.timeline_preview.as_deref()
            || self.snapshot.seeking
            || self.snapshot.buffering
            || self
                .timeline_loaded_at
                .is_some_and(|loaded| loaded.elapsed() < LOAD_GRACE)
        {
            return;
        }
        if preview::out_of_sync(frame.source, self.snapshot.position) {
            self.send(Command::SeekAbsolute(source));
            self.timeline_loaded_at = Some(Instant::now());
        }
    }

    /// 方向键在工程模式下挪播放头，而不是让 mpv 自己跳（它跳的是当前那一个素材）。
    pub(super) fn seek_timeline_by(&mut self, delta: f64, cx: &mut Context<Self>) {
        self.stop_timeline_playback(cx);
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let duration = project.sequence.duration();
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
    pub(super) fn step_timeline_frame(&mut self, steps: i32, cx: &mut Context<Self>) {
        self.stop_timeline_playback(cx);
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let fps = project.sequence.fps;
        let duration = project.sequence.duration();
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

    /// 播放时让播放头留在视口里：跑到右边 85% 之后，把视口往前推。
    ///
    /// 每帧都居中会让整个画面一直在动，看不清；只在快出界时往前赶一段，
    /// 播放头就可以一直在屏幕上走。
    fn follow_playhead(&mut self) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let playhead = project.ui.playhead.secs();
        let duration = project.sequence.duration();
        let width = self.timeline.width();
        let x = self.timeline.x_of(playhead);
        if x >= 0.0 && x <= width * 0.85 {
            return;
        }
        let visible = self.timeline.visible_seconds();
        self.timeline.scroll_to(playhead - visible * 0.2, duration);
    }
}
