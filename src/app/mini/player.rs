//! 小窗实体本身：字段、构造、帧交换、置顶、召回与状态同步。

use super::*;

/// 小窗：主窗口之外浮着的一个小播放器窗口。
///
/// 为什么不另起一个 mpv 实例：那样要多一份解码开销，音频会同时响两遍，
/// 两边的进度也会各自漂移。这里只显示主窗口转发过来的同一张帧，
/// 解码、音频、进度都还是主窗口那一份。
pub(in crate::app) struct MiniPlayer {
    /// 主窗口的视图。用弱引用：它关掉之后这里要能察觉到并跟着退出。
    pub(in crate::app) app: WeakEntity<PlayerApp>,
    /// 标题、进度等由主窗口推送的显示状态。
    pub(in crate::app) state: MiniState,
    pub(in crate::app) frame: Option<Arc<RenderImage>>,
    pub(in crate::app) incoming: Option<Arc<RenderImage>>,
    /// 已经画过、等待归还给 GPU atlas 的旧帧。规则与主窗口一致（留两帧余量），
    /// 但两窗口的 atlas 是各一份，所以各自还各自的。
    pub(in crate::app) retired: VecDeque<Arc<RenderImage>>,
    /// 鼠标是否停在小窗上。
    pub(in crate::app) hovered: bool,
    /// 悬浮条与控制栏是否可见。比 `hovered` 慢半拍，见 `set_hovered`。
    pub(in crate::app) controls: bool,
    /// 拖动进度条时的临时位置；`None` 表示没在拖。
    pub(in crate::app) dragging: Option<f64>,
    /// 拖动音量滑块时为真。
    pub(in crate::app) volume_dragging: bool,
    /// 音量滑块是否展开。
    pub(in crate::app) volume_open: bool,
    /// 指针停在音量图标上。
    pub(in crate::app) volume_hovered: bool,
    /// 指针停在音量面板上。图标与面板是两个独立的命中区域，少了这个标志，
    /// 指针一挪到面板上「离开图标」就会把面板收掉。
    pub(in crate::app) volume_panel_hovered: bool,
    /// 进度条与音量滑块的 bounds，由 canvas 在 prepaint 阶段写回。
    pub(in crate::app) progress_bounds: Rc<Cell<Bounds<Pixels>>>,
    pub(in crate::app) volume_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// 用户想不想让小窗置顶。写在偏好里，下次开小窗照它来。
    pub(in crate::app) on_top: bool,
    /// 已经下发给窗口系统的置顶状态；`None` 表示还没下发过。
    pub(in crate::app) on_top_applied: Option<bool>,
    /// 上一次记下的窗口宽度，用来判断用户有没有拖动过边框（变了才回写偏好）。
    pub(in crate::app) last_width: f32,
    /// 小窗所在的屏幕区域（全局坐标）。窗口位置要相对它换算成比例。
    pub(in crate::app) screen: Bounds<Pixels>,
    /// 上一次记下的位置比例；`None` 表示还没量过（打开后的第一帧只记基准）。
    pub(in crate::app) last_pos: Option<(f32, f32)>,
    /// 主窗口要求关掉小窗（或者是主窗口没了）。真正的关闭要等到下一次 render，
    /// 那时才拿得到 `&mut Window`。
    pub(in crate::app) dismissed: bool,
    /// 主窗口已经被释放，跟着退出。
    pub(in crate::app) app_gone: bool,
    /// 右键菜单的弹出位置（窗口坐标）；`None` 表示没开。
    pub(in crate::app) menu: Option<Point<Pixels>>,
    /// 已经下发给窗口的标题。任务栏按钮显示的就是它，变了才重设。
    pub(in crate::app) window_title: String,
    /// 临时浮层的文字（切集标题、循环模式）；`None` 表示不显示，2 秒后自动收回。
    pub(in crate::app) flash: Option<SharedString>,
    /// 浮层代次：连续触发时会起多个定时器，旧的那个不能把新浮层提前收掉。
    pub(in crate::app) flash_seq: u64,
    pub(in crate::app) focus: FocusHandle,
}

impl MiniPlayer {
    pub(in crate::app) fn new(
        app: WeakEntity<PlayerApp>,
        state: MiniState,
        on_top: bool,
        screen: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            app,
            state,
            frame: None,
            incoming: None,
            retired: VecDeque::new(),
            hovered: false,
            controls: false,
            dragging: None,
            volume_dragging: false,
            volume_open: false,
            volume_hovered: false,
            volume_panel_hovered: false,
            progress_bounds: Rc::new(Cell::new(Bounds::default())),
            volume_bounds: Rc::new(Cell::new(Bounds::default())),
            on_top,
            on_top_applied: None,
            last_width: 0.0,
            screen,
            last_pos: None,
            dismissed: false,
            app_gone: false,
            menu: None,
            window_title: String::new(),
            flash: None,
            flash_seq: 0,
            focus: cx.focus_handle(),
        }
    }

    /// 换片 / 停止：正在显示的和还在排队的帧都属于上一部片子。
    pub(in crate::app) fn discard_frame(&mut self) {
        self.incoming = None;
        if let Some(previous) = self.frame.take() {
            self.retired.push_back(previous);
        }
    }

    /// 换上刚到的那一帧，旧的排队等着还给 atlas。
    pub(super) fn swap_frame(&mut self) {
        let Some(incoming) = self.incoming.take() else {
            return;
        };
        if let Some(previous) = self.frame.replace(incoming) {
            self.retired.push_back(previous);
        }
    }

    /// 归还不再显示的帧；没有新帧送来时也要跑，清掉的旧帧才不会一直占着 atlas。
    pub(super) fn drop_retired_frames(&mut self, window: &mut Window) {
        while self.retired.len() > 2 {
            if let Some(stale) = self.retired.pop_front() {
                let _ = window.drop_image(stale);
            }
        }
    }

    /// 小窗一律置顶——不置顶它就被别的窗口盖住了，弹出来等于没弹。
    ///
    /// 与主窗口的 `sync_on_top` 各记各的：这是两个窗口，层级互不相关。
    pub(super) fn sync_on_top(&mut self, window: &Window, cx: &mut Context<Self>) {
        let want = self.on_top;
        if self.on_top_applied == Some(want) {
            return;
        }
        self.on_top_applied = Some(want);
        window.defer(cx, move |window, _cx| {
            if !platform::topmost::set(window, want) {
                log_warn!("小窗置顶未生效：当前窗口系统不支持");
            }
        });
    }

    /// 点到小窗就把它抬到普通窗口层的最前。
    ///
    /// 光靠系统默认的点击激活不够确定：被别的应用盖住时，激活并不总会把它抬到
    /// 那一层的最前，于是「点了它还是被压着」。这里显式改一次 Z 序。
    /// 置顶状态下不用管——它本来就在所有普通窗口之上。
    pub(super) fn raise_to_top(&self, window: &Window) {
        if self.on_top {
            return;
        }
        platform::topmost::raise(window);
    }

    /// 图钉按钮。切完立刻下发，不等下一次 render——置顶是个立刻能感觉到的动作。
    pub(super) fn toggle_on_top(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.on_top = !self.on_top;
        let want = self.on_top;
        // 顺手把 `on_top_applied` 记上，免得下一帧的 `sync_on_top` 又下发一遍。
        self.on_top_applied = Some(want);
        if !platform::topmost::set(window, want) {
            log_warn!("小窗置顶切换未生效：当前窗口系统不支持");
        }
        self.forward(cx, move |app, _cx| {
            app.settings.mini_on_top = want;
            app.settings.save();
        });
        cx.notify();
    }

    /// 小窗被系统收起来时把自己叫回来。
    ///
    /// **只在「小窗没有任务栏入口」的平台注册**（见 `open_mini`）。小窗是工具窗口
    /// 时，系统对「整个应用」下手——最小化的主窗口本来就收着，唯一的变化就是小窗
    /// 一起消失——屏幕上于是什么都不剩，点任务栏反而成了关掉播放器的开关。
    ///
    /// Windows 上不注册：那里小窗自己就是任务栏那一条，收起/还原都归用户说了算，
    /// 再插手只会让「点任务栏最小化」「显示桌面」变成一眨眼就弹回来的闪烁。
    ///
    /// 三条主动退出的路径（还原按钮 / T / Esc）会先置上 `dismissed`，应用退出时
    /// `app_gone` 会置上，两者都不召回，不会和销毁过程抢窗口。
    pub(in crate::app) fn recall(
        &mut self,
        visibility: WindowVisibility,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(visibility, WindowVisibility::Hidden) || self.dismissed || self.app_gone {
            return;
        }
        log_info!("小窗被系统收起，已重新召回");
        // 走 `bring_to_front` 而不是 `activate_window`：后者只在窗口「最小化」时
        // 才 `SW_RESTORE`，被 `SW_HIDE` 藏起来的那一路拉不回来。
        if !platform::window::bring_to_front(window) {
            window.activate_window();
        }
        cx.notify();
    }

    /// 从主窗口拉一份显示状态过来。由 `cx.observe` 在主窗口 notify 之后调用，
    /// 那时两边都没被借用，可以安全 `read`（原因见 `MiniState` 的文档）。
    pub(in crate::app) fn pull_state(&mut self, app: &Entity<PlayerApp>, cx: &mut Context<Self>) {
        let state = app.read(cx).mini_state();
        if self.state.same_as(&state) {
            return;
        }
        // 标题变了就是切了集。悬浮条在指针移开后 500ms 就收，用户看不到换了什么。
        // 不能判标题是否为空：`mini_state` 在空标题时填的是「小窗」，条件会恒真。
        if !state.empty && self.state.title != state.title {
            self.flash_text(state.title.clone(), cx);
        }
        // 循环模式在右键菜单里切，小窗没有别的反馈，也浮一条模式名。
        if self.state.loop_mode != state.loop_mode {
            self.flash_text(SharedString::from(state.loop_mode.label()), cx);
        }
        // 倍速由 `[` / `]` 调，小窗没有速度表，浮一条当前值。
        if (self.state.speed - state.speed).abs() >= 0.001 {
            self.flash_text(SharedString::from(format_speed(state.speed)), cx);
        }
        // 静音切换与滚轮调音量要有反馈：小窗盖在画面上，主窗口的 flash 看不见。
        if self.state.muted != state.muted {
            self.flash_text(
                SharedString::from(if state.muted {
                    "已静音"
                } else {
                    "取消静音"
                }),
                cx,
            );
        }
        // 拖动滑块时不浮——面板里本来就有数字，而且每帧都在变，浮层会被反复重启。
        if !self.volume_dragging && (self.state.volume - state.volume).abs() >= 0.5 {
            self.flash_text(SharedString::from(format!("音量 {:.0}%", state.volume)), cx);
        }
        self.state = state;
        cx.notify();
    }

    /// 让浮层显示 2 秒。代次用于连续触发：上一个定时器不能把新浮层提前收掉。
    pub(super) fn flash_text(&mut self, text: SharedString, cx: &mut Context<Self>) {
        self.flash_seq = self.flash_seq.wrapping_add(1);
        let seq = self.flash_seq;
        self.flash = Some(text);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            this.update(cx, |mini, cx| {
                if mini.flash_seq != seq || mini.flash.is_none() {
                    return;
                }
                mini.flash = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 把动作转给主窗口：播放器状态只有一份，小窗自己不持有 mpv 句柄。
    pub(super) fn forward(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut PlayerApp, &mut Context<PlayerApp>),
    ) {
        if let Some(app) = self.app.upgrade() {
            app.update(cx, f);
        }
    }

    /// 回到主窗口。先拉回主窗口再关自己：反过来的话中间那几帧屏幕上什么都没有。
    ///
    /// 悬浮条的还原按钮、T、Esc 与右键菜单都走这里，免得四份实现慢慢分叉。
    ///
    /// 宽度与位置在这里一并交给主窗口，而不是等 `mini_closed` 回头读小窗：
    /// 关闭是同步的，回调里小窗可能正被这次窗口更新租借着，再 `read` 会 panic。
    pub(super) fn restore_to_main(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismissed = true;
        let width = self.last_width;
        let pos = self.last_pos.map(|(x, y)| [x, y]);
        self.forward(cx, move |app, cx| {
            if width >= MINI_MIN_WIDTH {
                app.settings.mini_width = width;
            }
            if let Some(pos) = pos {
                app.settings.mini_pos = Some(pos);
            }
            app.reactivate_main(cx);
        });
        window.remove_window();
        cx.notify();
    }

    /// 尺寸变了就记进偏好，下次开小窗按用户拖过的宽度还原。
    ///
    /// 回写推到 defer 里而不是当场改：这段代码跑在 render 中，小窗的第一次绘制
    /// 又是从主窗口的回调里发起的，此刻 `PlayerApp` 正被可变借用，直接改它会 panic。
    /// defer 不保证在窗口销毁前一定跑到，所以 `mini_closed` 还会拿 `last_width`
    /// 兜一次底。
    pub(super) fn remember_width(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let width = f32::from(window.viewport_size().width);
        if (width - self.last_width).abs() < 1.0 {
            return;
        }
        self.last_width = width;
        let Some(app) = self.app.upgrade() else {
            return;
        };
        window.defer(cx, move |_window, cx| {
            app.update(cx, |app, _cx| app.settings.mini_width = width);
        });
    }

    /// 位置变了就按屏幕比例记进偏好，下次开小窗还落在同一个角落。
    ///
    /// 与 `remember_width` 同理：回写推到 defer，避免 render 期间二次租借
    /// `PlayerApp`。第一帧只记基准——位置是 `open_mini` 按偏好摆好的，不必回写。
    pub(super) fn remember_position(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 窗口可能被拖到另一块屏：比例要按它当前所在的那块算，否则会按旧屏的
        // 尺寸夹取，下次打开会被拉回旧屏边缘。
        let switched = match window.display(cx) {
            Some(display) => {
                let bounds = display.bounds();
                let switched =
                    bounds.origin != self.screen.origin || bounds.size != self.screen.size;
                if switched {
                    self.screen = bounds;
                }
                switched
            }
            None => false,
        };

        let bounds = window.bounds();
        let ratio = (
            geometry::axis_ratio(
                f32::from(bounds.origin.x),
                f32::from(self.screen.origin.x),
                f32::from(self.screen.size.width),
                f32::from(bounds.size.width),
            ),
            geometry::axis_ratio(
                f32::from(bounds.origin.y),
                f32::from(self.screen.origin.y),
                f32::from(self.screen.size.height),
                f32::from(bounds.size.height),
            ),
        );
        let changed = match self.last_pos {
            Some(last) => (last.0 - ratio.0).abs() >= 0.002 || (last.1 - ratio.1).abs() >= 0.002,
            None => false,
        };
        self.last_pos = Some(ratio);
        // 换屏时即使比例没变也要回写：同一个比例在新屏上对应的是另一个位置。
        if !switched && !changed {
            return;
        }
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let value = [ratio.0, ratio.1];
        window.defer(cx, move |_window, cx| {
            app.update(cx, |app, _cx| app.settings.mini_pos = Some(value));
        });
    }

    /// 鼠标进出。控件不是跟着指针立刻闪，而是移开之后再等 500ms：
    /// 从画面挪到底部控制栏的无主区域、或者贴边划过，都不该让控件抖一下。
    pub(super) fn set_hovered(&mut self, hovered: bool, cx: &mut Context<Self>) {
        self.hovered = hovered;
        if hovered {
            if !self.controls {
                self.controls = true;
            }
            cx.notify();
            return;
        }

        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            this.update(cx, |mini, cx| {
                // 期间指针又回来了就什么都不做。
                if mini.hovered || !mini.controls {
                    return;
                }
                mini.controls = false;
                mini.volume_open = false;
                mini.volume_hovered = false;
                mini.volume_panel_hovered = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
