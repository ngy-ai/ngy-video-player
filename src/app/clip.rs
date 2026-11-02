//! 裁切片段对话框：两个时间输入框、进度与完成后的操作。
//!
//! 输入框是自绘的：播放器整套界面都是 `div` 拼的，为两个只输数字的字段引入组件库
//! 的输入框，样式和主题都要跟着搬，不划算。代价是没有选区与输入法——时间只需要
//! 数字、冒号和小数点，够用。

use super::*;
use crate::clip::{
    self as clip_core, ClipEvent, ClipJob, ClipOutcome, ClipRange, ClipRequest, RangeError,
};

/// 输入框里最多放得下 `01:02:03.500` 这样的内容。
const MAX_INPUT_CHARS: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClipField {
    Start,
    End,
}

/// 对话框当前处在哪一步。
#[derive(Clone, Debug)]
enum ClipState {
    Editing,
    Running { progress: f32 },
    Done(PathBuf),
    Failed(String),
}

/// 裁切面板的全部状态。
pub(super) struct ClipDialog {
    /// 任务代次：后台任务回来时用它确认「这个事件还属于当前对话框」。
    generation: u64,
    path: PathBuf,
    duration: f64,
    start: String,
    end: String,
    /// 当前有输入焦点的字段；`None` 表示键盘归播放器。
    ///
    /// 面板不是模态的：没点进输入框时，空格、方向键必须照常控制播放——
    /// 用户要一边拖主进度条一边看画面、听声音来定位置。
    field: Option<ClipField>,
    state: ClipState,
    job: Option<Arc<ClipJob>>,
    /// 流复制吸附到关键帧后的实际起点；与输入相同时为 `None`。
    ///
    /// 无损剪切只能从关键帧起步，用户输入的时间几乎总落在两个关键帧之间，
    /// 这个差值必须显示出来，否则用户会以为剪切位置不准。
    aligned: Option<f64>,
    /// 导出参数：剪切方式、编码、滤镜与轨道保留。
    options: clip_core::ExportOptions,
    /// 正在预览片段时的终点与开始时刻（秒）。
    ///
    /// 记下开始时刻是为了躲开 seek 的延迟：刚跳到起点时位置事件可能还是旧的
    /// （甚至已经超过终点），不能拿它判断「播到结尾了」。
    preview: Option<(f64, Instant)>,
}

impl ClipDialog {
    fn new(generation: u64, path: PathBuf, duration: f64) -> Self {
        Self {
            generation,
            path,
            duration,
            start: String::new(),
            end: String::new(),
            field: None,
            state: ClipState::Editing,
            job: None,
            aligned: None,
            options: clip_core::ExportOptions::default(),
            preview: None,
        }
    }

    fn editing(&self) -> bool {
        matches!(self.state, ClipState::Editing | ClipState::Failed(_))
    }

    /// 当前输入能构成什么区间。`Ok(None)` 表示两端都留空。
    fn range(&self) -> Result<Option<ClipRange>, RangeError> {
        clip_core::range_from_inputs(&self.start, &self.end, self.duration)
    }

    /// 有焦点的输入框；`None` 时不该有键盘输入进来。
    fn active_text_mut(&mut self) -> Option<&mut String> {
        match self.field? {
            ClipField::Start => Some(&mut self.start),
            ClipField::End => Some(&mut self.end),
        }
    }
}

impl PlayerApp {
    /// 打开对话框。没有正在播放的文件、或者时长还没读出来时，只给一条提示。
    pub(super) fn open_clip_dialog(&mut self, cx: &mut Context<Self>) {
        // 再点一次菜单就是收起：面板不挡播放，开关语义比「重新开一个」自然。
        if self.clip.is_some() {
            self.close_clip_dialog(cx);
            return;
        }
        let Some(path) = self.snapshot.path.clone() else {
            self.flash("没有正在播放的文件");
            cx.notify();
            return;
        };
        let duration = self.snapshot.duration;
        if duration <= 0.0 {
            self.flash("还没读出这个文件的时长，稍等一下再试");
            cx.notify();
            return;
        }

        // 轨道编辑与元数据编辑都是模态的，叠在面板上会挡住画面与进度条。
        self.close_remux_dialog(cx);
        self.close_metadata_dialog(cx);
        self.close_proxy_dialog(cx);
        self.close_scope_dialog(cx);
        self.close_multicam_dialog(cx);
        // 转换面板与裁切抢同一条右侧通道，先让位（正在跑的任务一并取消）。
        self.dismiss_convert_panel();
        self.clip_generation += 1;
        self.clip = Some(ClipDialog::new(self.clip_generation, path, duration));
        // 播放列表与裁切面板抢同一条右侧通道，先让位。
        self.set_playlist_visible(false);
        self.menu = None;
        self.menu_bar = None;
        cx.notify();
    }

    /// 关闭面板。正在跑的任务一并取消，半成品由任务线程删掉。
    pub(super) fn close_clip_dialog(&mut self, cx: &mut Context<Self>) {
        self.dismiss_clip_panel();
        cx.notify();
    }

    /// 换片后裁切面板不能再留：它的时长与路径都来自打开时那一个文件，
    /// 继续用会把新片的播放位置写进旧片的区间。由 `render` 每帧对照一次。
    pub(super) fn sync_clip_source(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.clip.as_ref() else {
            return;
        };
        if self.snapshot.path.as_deref() != Some(dialog.path.as_path()) {
            self.close_clip_dialog(cx);
        }
    }

    /// 收面板但不刷新。给同一帧里还要继续改状态的调用方（切播放列表）。
    pub(super) fn dismiss_clip_panel(&mut self) {
        if let Some(dialog) = self.clip.take()
            && let Some(job) = &dialog.job
        {
            job.cancel();
        }
    }

    /// 输入框有焦点时才接管键盘，返回 `true` 表示这次按键已被消费。
    ///
    /// 面板不再是模态的：没点进输入框时，空格、方向键、Ctrl/⌘+左右这些必须原样
    /// 留给播放器——用户正是一边看画面一边用它们定位。
    pub(super) fn on_clip_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let Some(dialog) = self.clip.as_ref() else {
            return false;
        };
        if dialog.field.is_none() || !dialog.editing() {
            return false;
        }

        match event.keystroke.key.as_str() {
            // Esc 先退出输入焦点，再按一次才收面板（走 ExitOverlay）。
            "escape" => {
                if let Some(dialog) = self.clip.as_mut() {
                    dialog.field = None;
                }
                cx.notify();
                true
            }
            "tab" | "down" => {
                self.clip_field_switch(cx, false);
                true
            }
            "up" => {
                self.clip_field_switch(cx, true);
                true
            }
            "enter" => {
                self.start_clip(cx);
                true
            }
            "backspace" => {
                if let Some(dialog) = self.clip.as_mut()
                    && let Some(text) = dialog.active_text_mut()
                {
                    text.pop();
                    cx.notify();
                }
                true
            }
            _ => {
                let typed = event.keystroke.key_char.as_deref().filter(|text| {
                    !text.is_empty()
                        && text
                            .chars()
                            .all(|ch| ch.is_ascii_digit() || ch == ':' || ch == '.')
                });
                let Some(text) = typed else {
                    // 空格、方向键之类不参与时间输入，交还给播放器。
                    return false;
                };
                if let Some(dialog) = self.clip.as_mut()
                    && let Some(target) = dialog.active_text_mut()
                    && target.len() + text.len() <= MAX_INPUT_CHARS
                {
                    target.push_str(text);
                }
                cx.notify();
                true
            }
        }
    }

    fn clip_field_switch(&mut self, cx: &mut Context<Self>, backwards: bool) {
        let Some(dialog) = self.clip.as_mut() else {
            return;
        };
        dialog.field = match (dialog.field, backwards) {
            (Some(ClipField::Start), false) | (Some(ClipField::End), true) => Some(ClipField::End),
            _ => Some(ClipField::Start),
        };
        cx.notify();
    }

    /// 把播放器当前的位置写进开始/结束输入框。
    ///
    /// 面板与播放器共用主进度条：用户拖进度条、逐帧微调之后停在哪一帧，
    /// 「设为开始」就取哪一帧，不必在面板里再定位一次。
    fn clip_mark(&mut self, field: ClipField, cx: &mut Context<Self>) {
        let position = self
            .snapshot
            .position
            .clamp(0.0, self.snapshot.duration.max(0.0));
        let text = clip_core::format_time(position);
        let text: String = text.chars().take(MAX_INPUT_CHARS).collect();
        let Some(dialog) = self.clip.as_mut() else {
            return;
        };
        match field {
            ClipField::Start => dialog.start = text,
            ClipField::End => dialog.end = text,
        }
        // 焦点跟到刚填的字段，接着用键盘微调或直接改都顺手。
        dialog.field = Some(field);
        cx.notify();
    }

    /// 预览选定区间：从开始播到结束，带声音，方便用耳朵和眼睛确认起止点。
    fn clip_preview_range(&mut self, cx: &mut Context<Self>) {
        let range = match self.clip.as_ref().map(|dialog| dialog.range()) {
            Some(Ok(Some(range))) => range,
            Some(Err(error)) => {
                self.flash(error.message());
                cx.notify();
                return;
            }
            _ => {
                self.flash("先设置开始和结束，才能预览片段");
                cx.notify();
                return;
            }
        };
        if let Some(dialog) = self.clip.as_mut() {
            dialog.preview = Some((range.end, Instant::now()));
        }
        self.snapshot.paused = false;
        self.send(Command::SeekAbsolute(range.start));
        self.send(Command::SetPaused(false));
        self.poke();
        cx.notify();
    }

    /// 预览播到终点就自动停下。由 `render` 每帧检查——播放中画面刷新会带着它走。
    pub(super) fn poll_clip_preview(&mut self, cx: &mut Context<Self>) {
        let Some((end, started)) = self.clip.as_ref().and_then(|dialog| dialog.preview) else {
            return;
        };
        // 用户自己按了暂停：不再替他停第二次，也不再跟踪终点。
        if self.snapshot.paused {
            if let Some(dialog) = self.clip.as_mut() {
                dialog.preview = None;
            }
            return;
        }
        // 刚发出 seek 时位置事件可能还是旧的，先给 seek 一点时间。
        if started.elapsed() < Duration::from_millis(250) || self.snapshot.position < end - 0.02 {
            return;
        }
        if let Some(dialog) = self.clip.as_mut() {
            dialog.preview = None;
        }
        self.snapshot.paused = true;
        self.send(Command::SetPaused(true));
        self.flash("预览结束");
        cx.notify();
    }

    /// 起一个后台任务。参数校验失败时留在编辑态，把原因显示在对话框里。
    pub(super) fn start_clip(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.clip.as_mut() else {
            return;
        };
        if !dialog.editing() {
            return;
        }
        let range = match dialog.range() {
            Ok(Some(range)) => range,
            // 两端都留空不是「复制一份」：按钮本来就是禁用的，这里再挡一次。
            Ok(None) | Err(_) => return,
        };

        let mut options = dialog.options.clone();
        if options.format == clip_core::OutputFormat::Hls {
            // HLS 的 TS 分片只装画面与音轨；界面上这两个开关会被隐藏，
            // 这里再兜一次底，防止旧状态带进来。
            options.keep_subtitles = false;
            options.keep_chapters = false;
        }
        let request = ClipRequest {
            output: clip_core::output_path(&dialog.path, options.format),
            input: dialog.path.clone(),
            range,
            options,
        };
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        match ClipJob::start(request, tx) {
            Ok(job) => {
                dialog.aligned = job.aligned;
                dialog.job = Some(Arc::new(job));
                dialog.state = ClipState::Running { progress: 0.0 };
                // 正在预览的那一段不该在裁切跑起来之后再把视频按停。
                dialog.preview = None;
            }
            // 进程都没起来，不会有任何事件，别去等一个空通道。
            Err(error) => {
                dialog.state = ClipState::Failed(format!("{error:#}"));
                cx.notify();
                return;
            }
        }
        let generation = dialog.generation;
        cx.notify();

        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let _ = this.update(cx, |this, cx| {
                    let Some(dialog) = this.clip.as_mut() else {
                        return;
                    };
                    // 旧任务可能在新对话框打开之后才收尾：只认当前代次。
                    if dialog.generation != generation {
                        return;
                    }
                    match event {
                        ClipEvent::Progress(progress) => {
                            if let ClipState::Running { progress: current } = &mut dialog.state {
                                *current = progress;
                            }
                        }
                        ClipEvent::Done(outcome) => {
                            dialog.job = None;
                            dialog.state = match outcome {
                                ClipOutcome::Finished(path) => ClipState::Done(path),
                                ClipOutcome::Cancelled => ClipState::Editing,
                                ClipOutcome::Failed(message) => ClipState::Failed(message),
                            };
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn clip_open_result(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.clip.as_ref() else {
            return;
        };
        let ClipState::Done(path) = &dialog.state else {
            return;
        };
        match platform::open::open(path) {
            Ok(()) => {
                let path = path.clone();
                self.flash(format!("已打开 {}", path.display()));
            }
            Err(message) => self.flash(message),
        }
        cx.notify();
    }

    fn clip_reveal_result(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.clip.as_ref() else {
            return;
        };
        let ClipState::Done(path) = &dialog.state else {
            return;
        };
        let path = path.clone();
        match platform::reveal::reveal(&path) {
            Ok(()) => self.flash("已在文件管理器里定位片段"),
            Err(message) => self.flash(message),
        }
        cx.notify();
    }

    /// 点面板以外的地方：输入框失焦，键盘还给播放器。返回是否真的清掉了焦点。
    pub(super) fn clear_clip_focus(&mut self) -> bool {
        let Some(dialog) = self.clip.as_mut() else {
            return false;
        };
        dialog.field.take().is_some()
    }
}

mod options;
mod panel;
