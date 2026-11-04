//! 播放控制与截图：播放/暂停、跳转、音量、倍速、截图卡片。

use super::*;

impl PlayerApp {
    pub(super) fn toggle_play(&mut self, cx: &mut Context<Self>) {
        // 工程模式下「播放」放的是时间线，不是 mpv 里那个文件：mpv 在这里只是
        // 预览的取帧器，让它自己往下放会直接跑过片段的出点、声音还对不上位置。
        if self.project.is_some() {
            self.toggle_timeline_playback(cx);
            self.poke();
            cx.notify();
            return;
        }
        self.send(Command::PlayPause);
        self.poke();
        cx.notify();
    }

    pub(super) fn seek_by(&mut self, delta: f64, cx: &mut Context<Self>) {
        // 工程模式跳的是播放头。发给 mpv 只会挪素材内部的时刻，播放头不动，
        // 松手之后画面又被下一次预览 seek 拉回去。
        if self.project.is_some() {
            self.seek_timeline_by(delta, cx);
            return;
        }
        self.send(Command::SeekRelative(delta));
        let arrow = if delta > 0.0 { "⏩" } else { "⏪" };
        self.flash(format!("{arrow} {:.0}s", delta.abs()));
        cx.notify();
    }

    pub(super) fn change_volume(&mut self, delta: f64, cx: &mut Context<Self>) {
        let next = (self.snapshot.volume + delta).clamp(0.0, 130.0);
        self.snapshot.volume = next;
        self.settings.volume = next;
        if self.snapshot.muted && next > 0.0 {
            self.snapshot.muted = false;
            self.send(Command::SetMuted(false));
        }
        self.send(Command::SetVolume(next));
        self.flash(format!("音量 {next:.0}%"));
        cx.notify();
    }

    /// 逐帧步进。`steps` 正数向前、负数向后。
    ///
    /// 看单帧时用户要的是「停在这一帧」，所以第一次滚轮就顺手暂停；本来就
    /// 暂停着则保持，不做 toggle，免得滚一下来回切。
    pub(super) fn step_frame(&mut self, steps: i32, cx: &mut Context<Self>) {
        // 工程模式走序列帧率：素材帧率可能和序列不一样，按素材走播放头就对不上
        // 片段边界，切点也会落在半帧上。
        if self.project.is_some() {
            self.step_timeline_frame(steps, cx);
            return;
        }
        if self.engine.is_none() || self.snapshot.idle {
            return;
        }
        if !self.snapshot.paused {
            self.snapshot.paused = true;
            self.send(Command::SetPaused(true));
        }
        self.send(Command::StepFrame(steps));
        let arrow = if steps > 0 { "⏭" } else { "⏮" };
        self.flash(format!("{arrow} 逐帧"));
        cx.notify();
    }

    pub(super) fn set_volume(&mut self, volume: f64) {
        let next = volume.clamp(0.0, 130.0);
        self.snapshot.volume = next;
        self.send(Command::SetVolume(next));
    }

    /// 调字幕延迟。`delta` 秒，正数延后、负数提前、0 归零。
    ///
    /// 修的是外挂字幕和片源差几秒这种常见问题（片头多了广告、帧率标错）。
    /// 值不落盘：它是这一部片子的属性，换片就该归零。
    pub(super) fn set_subtitle_delay(&mut self, delta: f64, cx: &mut Context<Self>) {
        let next = if delta.abs() < f64::EPSILON {
            0.0
        } else {
            // 一次 0.1 秒，浮点累加会跑出 0.30000000000000004，每步取整到一位。
            ((self.subtitle_delay + delta) * 10.0).round() / 10.0
        }
        .clamp(-60.0, 60.0);
        if (next - self.subtitle_delay).abs() < 0.001 {
            return;
        }
        self.subtitle_delay = next;
        self.send(Command::SetOption {
            name: "sub-delay".to_string(),
            value: format!("{next:.1}"),
        });
        self.flash(if next.abs() < 0.001 {
            "字幕延迟已归零".to_string()
        } else {
            format!("字幕延迟 {next:+.1}s")
        });
        cx.notify();
    }

    pub(super) fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        let muted = !self.snapshot.muted;
        self.snapshot.muted = muted;
        self.send(Command::SetMuted(muted));
        self.flash(if muted { "已静音" } else { "取消静音" });
        cx.notify();
    }

    /// 设置播放速度。
    ///
    /// 倍速是全局偏好：所有视频共用一个值，切歌不重置、下次启动照它恢复，
    /// 所以每次改动都立即落盘，不等窗口关闭——崩溃或被强杀时不能让用户白调。
    pub(super) fn set_speed(&mut self, speed: f64, cx: &mut Context<Self>) {
        let next = normalize_speed(speed);
        // 已经顶到量程两端时按快捷键不再产生变化：不必重发命令、不必重写配置。
        if (self.snapshot.speed - next).abs() < 0.001 && (self.settings.speed - next).abs() < 0.001
        {
            return;
        }
        self.snapshot.speed = next;
        self.settings.speed = next;
        self.settings.save();
        self.send(Command::SetSpeed(next));
        self.flash(format!("{next:.2}× 速度"));
        cx.notify();
    }

    /// 把自定义滑块的横坐标换算成倍速。
    ///
    /// 拖动过程里不弹 OSD：数字就在面板的气泡上，再盖一层反而挡住画面。
    /// 也不写配置：拖动中每移动一像素落一次盘太浪费，松手时才存（见鼠标抬起处）。
    pub(super) fn set_speed_from_x(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let bounds = self.speed_bounds.get();
        let width = f32::from(bounds.size.width) as f64;
        if width <= 1.0 {
            return;
        }
        let offset = (f32::from(x) - f32::from(bounds.origin.x)) as f64;
        let ratio = (offset / width).clamp(0.0, 1.0);
        let steps = (ratio * (SPEED_MAX - SPEED_MIN) / SPEED_STEP).round();
        let next = normalize_speed(SPEED_MIN + steps * SPEED_STEP);
        // 鼠标在同一个像素里抖动时别反复给 mpv 发同一条命令。
        if (self.snapshot.speed - next).abs() < 0.001 {
            return;
        }
        self.snapshot.speed = next;
        self.settings.speed = next;
        self.send(Command::SetSpeed(next));
        cx.notify();
    }

    pub(super) fn set_loop(&mut self, mode: LoopMode, cx: &mut Context<Self>) {
        self.settings.loop_mode = mode;
        self.flash(mode.label());
        cx.notify();
    }

    /// 让 mpv 抓当前画面存成 PNG。
    ///
    /// 不用界面里那一帧：它是按窗口尺寸缩过的，存档要的是原始分辨率。
    pub(super) fn take_screenshot(&mut self, cx: &mut Context<Self>) {
        if self.engine.is_none() {
            self.flash("截图不可用：播放引擎没有加载");
            cx.notify();
            return;
        }
        if !self.snapshot.has_video() {
            self.flash("没有可截图的画面");
            cx.notify();
            return;
        }

        let video = self.snapshot.path.clone();
        let target = match crate::screenshot::target(video.as_deref(), self.snapshot.position) {
            Ok(path) => path,
            Err(error) => {
                log_warn!("准备截图失败: {error:#}");
                self.flash(format!("截图失败：{error}"));
                cx.notify();
                return;
            }
        };
        self.send(Command::Screenshot(target.to_string_lossy().into_owned()));
        cx.notify();
    }

    /// 截图成功后弹出操作卡片，顺手在后台把截图缩成预览图。
    pub(super) fn show_screenshot_card(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        // 上一张卡片的缩略图已经进过图集，得走同一条回收路径，不能直接丢。
        self.clear_screenshot_card();
        self.screenshot_card = Some(ScreenshotCard {
            path: path.clone(),
            thumb: ScreenshotThumb::Loading,
            shown_at: Instant::now(),
            hovered: false,
        });

        // 解码要几十毫秒（4K 截图更久），放在主线程上会明显卡一下。
        let (tx, rx) = futures::channel::oneshot::channel();
        let target = path.clone();
        std::thread::spawn(move || {
            let _ = tx.send(crate::screenshot::thumbnail(
                &target,
                crate::screenshot::THUMBNAIL_MAX_WIDTH,
            ));
        });

        cx.spawn(async move |this, cx| {
            let thumb = match rx.await {
                Ok(Ok(image)) => ScreenshotThumb::Ready(image),
                Ok(Err(error)) => {
                    log_warn!("读取截图预览失败: {error:#}");
                    ScreenshotThumb::Failed
                }
                Err(_) => ScreenshotThumb::Failed,
            };
            let _ = this.update(cx, |this, cx| {
                // 解码期间可能又截了一张，或者卡片已经关掉：只认同一份。
                let Some(card) = this.screenshot_card.as_mut() else {
                    return;
                };
                if card.path != path {
                    return;
                }
                card.thumb = thumb;
                cx.notify();
            });
        })
        .detach();
    }

    /// 收起截图卡片。缩略图不能直接丢：它已经进过 GPU 图集，要交给渲染路径归还。
    pub(super) fn clear_screenshot_card(&mut self) {
        if let Some(card) = self.screenshot_card.take()
            && let ScreenshotThumb::Ready(thumb) = card.thumb
        {
            self.preview_retired.push_back(thumb);
        }
    }

    pub(super) fn copy_screenshot(&mut self, cx: &mut Context<Self>) {
        let Some(card) = &self.screenshot_card else {
            return;
        };
        let path = card.path.clone();
        match platform::clipboard::item(&path) {
            Ok(item) => {
                cx.write_to_clipboard(item);
                self.flash(platform::clipboard::COPIED_MESSAGE);
            }
            Err(error) => self.flash(format!("复制截图失败：{error:#}")),
        }
        cx.notify();
    }

    pub(super) fn open_screenshot(&mut self, cx: &mut Context<Self>) {
        let Some(card) = &self.screenshot_card else {
            return;
        };
        let path = card.path.clone();
        match platform::open::open(&path) {
            // 默认程序已经把窗口抢走了，卡片留着只会挡在新窗口后面。
            Ok(()) => self.clear_screenshot_card(),
            Err(message) => self.flash(message),
        }
        cx.notify();
    }

    pub(super) fn reveal_screenshot(&mut self, cx: &mut Context<Self>) {
        let Some(card) = &self.screenshot_card else {
            return;
        };
        let path = card.path.clone();
        match platform::reveal::reveal(&path) {
            Ok(()) => self.clear_screenshot_card(),
            Err(message) => self.flash(message),
        }
        cx.notify();
    }
}
