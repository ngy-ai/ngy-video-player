//! 播放控制与截图：播放/暂停、跳转、音量、倍速、截图卡片。

use super::*;

impl PlayerApp {
    pub(super) fn toggle_play(&mut self, cx: &mut Context<Self>) {
        self.send(Command::PlayPause);
        self.poke();
        self.sync_mini(cx);
        cx.notify();
    }

    pub(super) fn seek_by(&mut self, delta: f64, cx: &mut Context<Self>) {
        self.send(Command::SeekRelative(delta));
        let arrow = if delta > 0.0 { "⏩" } else { "⏪" };
        self.flash(format!("{arrow} {:.0}s", delta.abs()));
        self.sync_mini(cx);
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
        self.sync_mini(cx);
        cx.notify();
    }

    /// 逐帧步进。`steps` 正数向前、负数向后。
    ///
    /// 看单帧时用户要的是「停在这一帧」，所以第一次滚轮就顺手暂停；本来就
    /// 暂停着则保持，不做 toggle，免得滚一下来回切。
    pub(super) fn step_frame(&mut self, steps: i32, cx: &mut Context<Self>) {
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

    pub(super) fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        let muted = !self.snapshot.muted;
        self.snapshot.muted = muted;
        self.send(Command::SetMuted(muted));
        self.flash(if muted { "已静音" } else { "取消静音" });
        self.sync_mini(cx);
        cx.notify();
    }

    pub(super) fn set_speed(&mut self, speed: f64, cx: &mut Context<Self>) {
        let next = (speed * 100.0).round() / 100.0;
        self.snapshot.speed = next;
        self.settings.speed = next;
        self.send(Command::SetSpeed(next));
        self.flash(format!("{next:.2}× 速度"));
        cx.notify();
    }

    /// 把自定义滑块的横坐标换算成倍速。
    ///
    /// 拖动过程里不弹 OSD：数字就在面板的气泡上，再盖一层反而挡住画面。
    pub(super) fn set_speed_from_x(&mut self, x: Pixels, cx: &mut Context<Self>) {
        let bounds = self.speed_bounds.get();
        let width = f32::from(bounds.size.width) as f64;
        if width <= 1.0 {
            return;
        }
        let offset = (f32::from(x) - f32::from(bounds.origin.x)) as f64;
        let ratio = (offset / width).clamp(0.0, 1.0);
        let steps = (ratio * (SPEED_MAX - SPEED_MIN) / SPEED_STEP).round();
        let speed = (SPEED_MIN + steps * SPEED_STEP).clamp(SPEED_MIN, SPEED_MAX);
        let next = (speed * 100.0).round() / 100.0;
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
