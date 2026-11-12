//! 双画面修剪窗口：切点前后两帧 + 两帧的时间码。
//!
//! 取帧走 ffmpeg 单帧导出（`-ss` 输入定位 + `-frames:v 1` 到 stdout），跑在后台线程上：
//! 精确 seek 要解一段，冷启动 100–300ms，不能放在界面线程里。
//!
//! 窗口**在松手之后**才弹：拖动中每动一像素都去起一次 ffmpeg 会把机器拖垮，所以拖动
//! 过程只有轨道上的落点提示，松手才取两帧。点窗口任意位置关掉它。

use super::*;
use crate::platform::ffmpeg;
use crate::timeline::{Id, Tick, TrackKind};
use image::{Frame, ImageBuffer, Rgba};
use std::path::Path;
use std::process::Stdio;

/// 两帧缩略图的宽高（16:9）。窗口按它排，素材不是 16:9 时按宽度铺满、上下留黑。
const FRAME_WIDTH: f32 = 168.0;
const FRAME_HEIGHT: f32 = 94.5;

/// 双画面修剪窗口的状态。
pub(crate) struct TrimWindow {
    /// 正在剪的片段（片段被删掉就跟着收起）。
    pub(crate) clip: Id,
    pub(crate) side: super::timeline_tracks::TrimSide,
    /// 切点在序列上的时刻。
    pub(crate) edge: Tick,
    /// 两帧在素材内部的时刻：`[切点前一帧, 切点那一帧]`。
    pub(crate) times: [Tick; 2],
    pub(crate) frames: [Option<Arc<RenderImage>>; 2],
    pub(crate) loading: bool,
    /// 每次重开换一个号：晚到的两帧不能画到新窗口上。
    pub(crate) token: u64,
}

/// 切点前后两帧在**素材内部**的时刻。
///
/// 序列时间换成素材时间按 1:1（与 `ops` 的修剪换算同一套约定）：`edge` 相对片段起点的
/// 偏移直接加到 `source_in` 上。左端修剪的切点是新的入点，右端是新的出点。
pub(crate) fn frame_source_times(
    clip_start: Tick,
    source_in: Tick,
    edge: Tick,
    one_frame: Tick,
) -> [Tick; 2] {
    let offset = (edge - clip_start).max(Tick::ZERO);
    let cut = (source_in + offset).max(Tick::ZERO);
    [(cut - one_frame).max(Tick::ZERO), cut]
}

impl PlayerApp {
    /// 松手之后调：按新切点取两帧，弹窗口。
    ///
    /// 片段没画面（纯音频）时不弹——两帧全黑没有意义，提示条已经说了长度。
    pub(crate) fn open_trim_window(
        &mut self,
        clip: Id,
        side: super::timeline_tracks::TrimSide,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let sequence = project.sequence();
        let Some(item) = sequence.clip(clip) else {
            return;
        };
        if let Some(track_id) = sequence.clip_track(clip)
            && let Some(track) = sequence.track(track_id)
            && track.kind != TrackKind::Video
        {
            return;
        }
        let Some(path) = project
            .asset(item.asset_id)
            .map(|asset| asset.path.clone())
            .filter(|path| path.exists())
        else {
            return;
        };
        let one_frame = Tick::from_frames(1, sequence.fps);
        let edge = match side {
            super::timeline_tracks::TrimSide::Start => item.start,
            super::timeline_tracks::TrimSide::End => item.end(),
        };
        let times = frame_source_times(item.start, item.source_in, edge, one_frame);

        self.trim_token += 1;
        let token = self.trim_token;
        self.trim_window = Some(TrimWindow {
            clip,
            side,
            edge,
            times,
            frames: [None, None],
            loading: true,
            token,
        });

        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            // 两帧各起一次 ffmpeg：并行没有必要，串行也就几百毫秒，还少一个线程。
            let frames = [grab_frame(&path, times[0]), grab_frame(&path, times[1])];
            let _ = tx.send(frames);
        });
        cx.spawn(async move |this, cx| {
            let Ok(frames) = rx.await else {
                return;
            };
            this.update(cx, |this, cx| {
                if let Some(window) = this.trim_window.as_mut()
                    && window.token == token
                {
                    window.frames = frames;
                    window.loading = false;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn close_trim_window(&mut self, cx: &mut Context<Self>) {
        if self.trim_window.take().is_some() {
            cx.notify();
        }
    }

    /// 双画面窗口。浮在时间线左下角，点一下关掉。
    pub(crate) fn render_trim_window(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(project) = self.project.as_ref() else {
            return div().into_any_element();
        };
        let Some(window) = self.trim_window.as_ref() else {
            return div().into_any_element();
        };
        // 片段被删掉 / 切走序列之后窗口不该还挂着。
        if project.sequence().clip(window.clip).is_none() {
            return div().into_any_element();
        }
        let fps = project.sequence().fps;
        let drop_frame = crate::timeline::timecode::prefers_drop_frame(fps);
        let title = match window.side {
            super::timeline_tracks::TrimSide::Start => "修剪入点",
            super::timeline_tracks::TrimSide::End => "修剪出点",
        };
        let cut_note = format!(
            "{title} · 切点 {}",
            crate::timeline::timecode::format(window.edge, fps, drop_frame)
        );
        let labels = [
            format!(
                "前一帧 {}",
                crate::timeline::timecode::format(window.times[0], fps, drop_frame)
            ),
            format!(
                "后一帧 {}",
                crate::timeline::timecode::format(window.times[1], fps, drop_frame)
            ),
        ];

        let mut frames = div().flex().gap(px(6.0));
        for (index, label) in labels.into_iter().enumerate() {
            let body = match &window.frames[index] {
                Some(image) => img(image.clone())
                    .w(px(FRAME_WIDTH))
                    .h(px(FRAME_HEIGHT))
                    .into_any_element(),
                None => div()
                    .w(px(FRAME_WIDTH))
                    .h(px(FRAME_HEIGHT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(gpui_kit::rgb(0x15151c))
                    .text_size(px(10.0))
                    .text_color(text_faint())
                    .child(if window.loading {
                        "取帧中…"
                    } else {
                        "这一帧取不出来"
                    })
                    .into_any_element(),
            };
            frames = frames.child(
                div().flex().flex_col().gap(px(3.0)).child(body).child(
                    div()
                        .text_size(px(10.0))
                        .text_color(text_muted())
                        .child(label),
                ),
            );
        }

        div()
            .id("timeline-trim-window")
            .absolute()
            .left(px(TIMELINE_GUTTER + 12.0))
            // 压在轨道区下方一点：既不挡播放头，也在鼠标附近。
            .bottom(px(12.0))
            .p(px(8.0))
            .rounded(px(8.0))
            .bg(gpui_kit::rgb(0x1b1b24f5))
            .border_1()
            .border_color(gpui_kit::rgb(0x2c2c34))
            .shadow_lg()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                cx.stop_propagation();
                this.close_trim_window(cx);
            }))
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(ui::accent())
                    .child(cut_note),
            )
            .child(frames)
            .into_any_element()
    }
}

/// 用 ffmpeg 取素材内部某一个时刻的一帧，缩到面板宽度。
///
/// `-ss` 放在 `-i` 之前：ffmpeg 会从最近的关键帧开始解、再丢到目标时刻，比从 0 解
/// 快得多，精度仍是逐帧的（`-frames:v 1` 取第一帧）。
fn grab_frame(path: &Path, at: Tick) -> Option<Arc<RenderImage>> {
    let executable = ffmpeg::executable().ok()?;
    let output = std::process::Command::new(executable)
        .arg("-v")
        .arg("error")
        .arg("-ss")
        .arg(format!("{:.6}", at.secs().max(0.0)))
        .arg("-i")
        .arg(path)
        .arg("-frames:v")
        .arg("1")
        .arg("-vf")
        .arg(format!("scale={FRAME_WIDTH:.0}:-2"))
        .arg("-f")
        .arg("image2pipe")
        .arg("-vcodec")
        .arg("png")
        .arg("-")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }
    let decoded = image::load_from_memory(&output.stdout).ok()?.to_rgba8();
    let (width, height) = (decoded.width(), decoded.height());
    if width == 0 || height == 0 {
        return None;
    }
    // `RenderImage` 收 BGRA，`image` 给 RGBA：红蓝换过来（与封面、截图同一套约定）。
    let mut pixels = decoded.into_raw();
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let buffer: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_raw(width, height, pixels)?;
    Some(Arc::new(RenderImage::new([Frame::new(buffer)])))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(seconds: f64) -> Tick {
        Tick::from_secs(seconds)
    }

    #[test]
    fn 左端修剪取入点前后两帧() {
        // 片段从序列 5 秒开始、素材从 2 秒开始；左端收到 6 秒。
        let times = frame_source_times(s(5.0), s(2.0), s(6.0), s(0.04));
        assert_eq!(times[1], s(3.0), "切点对应素材 3 秒");
        assert_eq!(times[0], s(2.96), "前一帧是切点减一帧");
    }

    #[test]
    fn 右端修剪取末帧与出点() {
        // 片段 0–4 秒、素材 1–5 秒；右端收到 3 秒 → 素材 4 秒。
        let times = frame_source_times(Tick::ZERO, s(1.0), s(3.0), s(0.04));
        assert_eq!(times, [s(3.96), s(4.0)]);
    }

    #[test]
    fn 素材开头附近不会取到负时间() {
        // 入点就是素材第 0 帧、切点也在片段起点：前一帧只能是 0。
        let times = frame_source_times(s(2.0), Tick::ZERO, s(2.0), s(0.04));
        assert_eq!(times, [Tick::ZERO, Tick::ZERO]);
    }
}
