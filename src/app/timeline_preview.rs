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
//!
//! 按职责分文件：
//!
//! - 本文件：取帧与跟随（画面这一路），以及共享常量。
//! - `composite`：多轨合成预览的接管与交回。
//! - `playback`：J/K/L 变速、播放时钟、scrub 与播放头步进。
//! - `audio`：音频轨的多实例出声。

mod audio;
mod composite;
mod playback;

use std::time::{Duration, Instant};

use super::*;
use crate::timeline::preview;

pub(crate) use self::audio::TrackAudio;

/// 加载 / seek 之后的宽限期：这段时间里 mpv 的位置还没追上，不能拿来判断偏差。
///
/// 不宜太长：`Load` 带的 seek 只落到关键帧上，长 GOP 的素材可能从入点前一两秒
/// 开始放，宽限期一过就要靠偏差纠正补一次精确 seek，那之前看到的画面偏早。
const LOAD_GRACE: Duration = Duration::from_millis(300);

impl PlayerApp {
    /// 按当前播放头刷新预览画面（拖动、跳转之后）。
    pub(super) fn preview_at_playhead(&mut self, cx: &mut Context<Self>) {
        // 时间线一动，试听就结束：预览区的主语回到序列，而不是双击过的那个原片。
        // 三点编辑的源区间不跟着走（`source_asset` 另行记账）：用户定完源入出点，
        // 下一步本来就是点时间线定第三个点，这一步不能把他的源区间一起抹掉。
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
        // 声音和画面各走一路实例：音频轨上有片段时，这里也要跟着对位。
        self.sync_timeline_audio(at, playing, false);
        // 多轨叠加交给合成器：主实例只有一路画面，叠不出来。
        if self.present_composite(at, playing, true) {
            // 主实例让位：它再往界面上推帧就会盖掉合成结果。
            self.send(Command::SetPaused(true));
            return true;
        }
        self.leave_composite();
        let Some(project) = self.project.as_ref() else {
            return false;
        };
        let Some(frame) = preview::resolve(project.sequence(), at) else {
            // 空档的黑场由画面那一层压黑（`preview::overlay_dim`），这里只负责别出声：
            // 不暂停的话 mpv 会接着放上一段的声音，播放头走过空档时听着像窜了音。
            self.send(Command::SetPaused(true));
            return false;
        };
        // 缓存 / 代理 / 原片的选择在 `preview_media` 里；带缓存时时间换成片段内偏移。
        let Some((path, source_tick)) = self.preview_media(project, &frame, at) else {
            return false;
        };
        let source = source_tick.secs();

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
        // 定格段没有「接着走」这回事：停在最后一帧，等播放头走出定格再放。
        // 速度曲线的片段同理：mpv 只会按恒定速度放，放出去就跟播放头散了。
        //
        // 拖动时不按住：scrub 要的就是接着出声，收声交给拖动的尾巴计时器——
        // 每挪一下先 pause 再 unpause，光重启音频输出就够把 100 ms 的预算吃光。
        let pause = if playing {
            frame.frozen || frame.remap
        } else {
            !self.timeline_dragging
        };
        self.send(Command::SetPaused(pause));
        true
    }

    /// 播放中的画面跟随：只在换素材、走出空档、偏差过大时才给 mpv 下命令。
    ///
    /// 每个节拍都 seek 一次是最省事的写法，但 mpv 每次精确 seek 都要重新解码，
    /// 声音会一直打嗝。正常播放时它自己就在正确的速度上走，没有必要纠正。
    fn preview_follow(&mut self, at: Tick) {
        // 播放中的声音跟随：只在换进程内的片段时动手，不逐节拍 seek。
        self.sync_timeline_audio(at, true, true);
        // 合成器在管画面时，主实例不必再跟：两路 mpv 自己在按各自的位置放。
        if self.present_composite(at, true, false) {
            return;
        }
        self.leave_composite();
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let Some(frame) = preview::resolve(project.sequence(), at) else {
            // 空档：这一刻没有任何画面。停住 mpv，播放头照常往前走。
            if !self.snapshot.paused {
                self.send(Command::SetPaused(true));
            }
            return;
        };
        let Some((path, source_tick)) = self.preview_media(project, &frame, at) else {
            return;
        };
        let source = source_tick.secs();

        if self.timeline_preview.as_deref() != Some(path.as_path()) {
            self.timeline_preview = Some(path.clone());
            self.timeline_loaded_at = Some(Instant::now());
            self.begin_load(&path);
            self.send(Command::Load {
                url: path.to_string_lossy().into_owned(),
                start: Some(source),
            });
            self.send(Command::SetPaused(!frame.frozen));
            return;
        }
        // 定格段：mpv 自己会一路播到文件末尾，不按住它就会在定格里继续放后面的素材。
        // 只在它还走着的时候补一次 seek + 暂停；暂停生效之后每个节拍什么都不做。
        if frame.frozen {
            if !self.snapshot.paused {
                self.send(Command::SeekAbsolute(source));
                self.send(Command::SetPaused(true));
                self.timeline_loaded_at = Some(Instant::now());
            }
            return;
        }
        // 速度曲线：素材位置不再随时间线性走，交给 mpv 自己放必然越走越偏。
        // 每拍补一次 seek（与倒放同一条路径），代价是这一段在预览里没有声音——
        // 真正的变速出声只有导出有（分段 `atempo`）。
        if frame.remap {
            self.send(Command::SeekAbsolute(source));
            self.send(Command::SetPaused(true));
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
        if preview::out_of_sync(source_tick, self.snapshot.position) {
            self.send(Command::SeekAbsolute(source));
            self.timeline_loaded_at = Some(Instant::now());
        }
    }

    /// 播放时让播放头留在视口里：跑到右边 85% 之后，把视口往前推。
    ///
    /// 每帧都居中会让整个画面一直在动，看不清；只在快出界时往前赶一段，
    /// 播放头就可以一直在屏幕上走。
    pub(super) fn follow_playhead(&mut self) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let playhead = project.ui.playhead.secs();
        let duration = project.sequence().duration();
        let width = self.timeline.width();
        let x = self.timeline.x_of(playhead);
        if x >= 0.0 && x <= width * 0.85 {
            return;
        }
        let visible = self.timeline.visible_seconds();
        self.timeline.scroll_to(playhead - visible * 0.2, duration);
    }
}
