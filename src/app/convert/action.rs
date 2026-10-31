//! 转换面板的动作：开关面板、选文件夹、启动与取消批次。

use super::super::*;
use super::state::{ConvertDialog, ConvertSource, ConvertState, ScanState};
use crate::convert::{self, ConvertEvent, ConvertJob, ConvertOutcome, TargetFormat};

/// 搜索词长度上限：再长也不会更精确，只是占地方。
const MAX_QUERY_CHARS: usize = 24;

impl PlayerApp {
    /// 打开面板。再点一次菜单就是收起：面板不挡播放，开关语义比「重新开一个」自然。
    pub(in crate::app) fn open_convert_dialog(&mut self, cx: &mut Context<Self>) {
        if self.convert.is_some() {
            self.close_convert_dialog(cx);
            return;
        }
        // 轨道编辑是模态的，叠在面板上会挡住画面与进度条。
        self.close_remux_dialog(cx);
        // 裁切面板与播放列表跟转换抢同一条右侧通道，先让位。
        self.dismiss_clip_panel();
        self.convert_generation += 1;
        self.convert = Some(ConvertDialog::new(self.convert_generation));
        self.set_playlist_visible(false);
        self.menu = None;
        self.menu_bar = None;
        cx.notify();
    }

    /// 关闭面板。正在跑的批次一并取消，半成品由任务线程删掉。
    pub(in crate::app) fn close_convert_dialog(&mut self, cx: &mut Context<Self>) {
        self.dismiss_convert_panel();
        cx.notify();
    }

    /// 收面板但不刷新。给同一帧里还要继续改状态的调用方（切播放列表、开裁切）。
    pub(in crate::app) fn dismiss_convert_panel(&mut self) {
        if let Some(dialog) = self.convert.take()
            && let Some(job) = &dialog.job
        {
            job.cancel();
        }
    }

    /// 这一批要转哪些文件。扫描还没读出来时是空列表。
    pub(super) fn convert_inputs(&self) -> Vec<PathBuf> {
        let Some(dialog) = self.convert.as_ref() else {
            return Vec::new();
        };
        match (&dialog.source, &dialog.scan) {
            (ConvertSource::Current, _) => self.snapshot.path.clone().into_iter().collect(),
            (ConvertSource::Folder(_), ScanState::Ready(files)) => files.clone(),
            _ => Vec::new(),
        }
    }

    /// 这一批有多少个文件。
    ///
    /// 渲染每帧都会问一次，所以不克隆整张列表：几百项的文件夹每帧复制一遍路径没有意义。
    pub(super) fn convert_input_count(&self) -> usize {
        let Some(dialog) = self.convert.as_ref() else {
            return 0;
        };
        match (&dialog.source, &dialog.scan) {
            (ConvertSource::Current, _) => usize::from(self.snapshot.path.is_some()),
            (ConvertSource::Folder(_), ScanState::Ready(files)) => files.len(),
            _ => 0,
        }
    }

    /// 来源切回「当前文件」。
    pub(super) fn convert_use_current_file(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.convert.as_mut() {
            dialog.source = ConvertSource::Current;
            dialog.scan = ScanState::Idle;
        }
        cx.notify();
    }

    /// 来源切到「文件夹」；还没选过就直接弹选择框。
    pub(super) fn convert_use_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let already = self
            .convert
            .as_ref()
            .is_some_and(|dialog| matches!(dialog.source, ConvertSource::Folder(_)));
        if already {
            return;
        }
        self.convert_pick_folder(window, cx);
    }

    /// 选一个文件夹（含子文件夹）作为来源。选中后立刻在后台线程里扫描。
    pub(super) fn convert_pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let mut dialog = rfd::FileDialog::new().set_parent(window);
        if let Some(dir) = &self.settings.last_dir {
            dialog = dialog.set_directory(dir);
        }
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(dialog.pick_folder());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if let Some(folder) = picked {
                    this.remember_dir(&folder);
                    this.convert_start_scan(folder, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 后台扫描文件夹。代次一变，之前那次扫描的结果就作废。
    fn convert_start_scan(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        self.convert_generation += 1;
        let generation = self.convert_generation;
        if let Some(dialog) = self.convert.as_mut() {
            // 换来源时正跑着的批次必须停：它的输出路径是按旧列表算的。
            if let Some(job) = dialog.job.take() {
                job.cancel();
            }
            dialog.generation = generation;
            dialog.source = ConvertSource::Folder(folder.clone());
            dialog.scan = ScanState::Scanning;
            dialog.state = ConvertState::Editing;
        }
        cx.notify();

        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            // 几百个文件的目录要读一会儿，不能放在界面线程上。
            let _ = tx.send(convert::scan_folder(&folder));
        });

        cx.spawn(async move |this, cx| {
            // 通道断了（线程没起来）也当成读不了，总比一直停在「正在扫描」强。
            let scanned = rx
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("扫描线程提前退出")));
            let _ = this.update(cx, |this, cx| {
                let Some(dialog) = this.convert.as_mut() else {
                    return;
                };
                // 用户可能已经换了文件夹（甚至关了面板）：只认当前代次。
                if dialog.generation != generation {
                    return;
                }
                dialog.scan = match scanned {
                    Ok(files) => ScanState::Ready(files),
                    Err(error) => ScanState::Failed(format!("{error:#}")),
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// 展开 / 收起格式下拉。展开时清空搜索词，从完整列表开始。
    pub(super) fn convert_toggle_picker(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.convert.as_mut() {
            dialog.picker.open = !dialog.picker.open;
            dialog.picker.query.clear();
        }
        cx.notify();
    }

    pub(in crate::app) fn convert_close_picker(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.convert.as_mut() {
            dialog.picker.open = false;
            dialog.picker.query.clear();
        }
        cx.notify();
    }

    /// 选中一个格式并收起列表。
    pub(super) fn convert_pick_format(&mut self, target: TargetFormat, cx: &mut Context<Self>) {
        if let Some(dialog) = self.convert.as_mut() {
            dialog.target = target;
            dialog.picker.open = false;
            dialog.picker.query.clear();
            // 上一批的失败原因是针对旧格式的，换了格式就该清掉。
            if matches!(dialog.state, ConvertState::Failed(_)) {
                dialog.state = ConvertState::Editing;
            }
        }
        cx.notify();
    }

    /// 下拉展开时接管键盘，返回 `true` 表示这次按键已被消费。
    ///
    /// 搜索框是自绘的（跟裁切的时间输入框同一套做法）：没有选区、没有输入法，
    /// 光标永远在末尾。展开期间字母键必须归搜索框，否则会顺带触发播放器快捷键。
    pub(in crate::app) fn on_convert_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(dialog) = self.convert.as_ref() else {
            return false;
        };
        if !dialog.picker.open || !dialog.editing() {
            return false;
        }
        let keystroke = &event.keystroke;
        // 带 Ctrl / Alt / ⌘ 的组合留给全局快捷键（保存、全屏这些不该被搜索框吃掉）。
        if keystroke.modifiers.control || keystroke.modifiers.alt || keystroke.modifiers.platform {
            return false;
        }

        match keystroke.key.as_str() {
            // Esc 先收列表，再按一次才收面板（走 ExitOverlay）。
            "escape" => self.convert_close_picker(cx),
            // 回车选中第一个匹配项：输完几个字母直接回车是最快的路径。
            "enter" => {
                if let Some(target) = self
                    .convert
                    .as_ref()
                    .and_then(|dialog| dialog.matches().first().copied())
                {
                    self.convert_pick_format(target, cx);
                }
            }
            "backspace" => {
                if let Some(dialog) = self.convert.as_mut() {
                    dialog.picker.query.pop();
                }
                cx.notify();
            }
            _ => {
                let typed = keystroke
                    .key_char
                    .as_deref()
                    .filter(|text| !text.is_empty() && text.chars().all(|ch| !ch.is_control()));
                let Some(text) = typed else {
                    // 方向键、F1 之类不参与搜索，交还给播放器。
                    return false;
                };
                if let Some(dialog) = self.convert.as_mut()
                    && dialog.picker.query.chars().count() < MAX_QUERY_CHARS
                {
                    dialog.picker.query.push_str(text);
                }
                cx.notify();
            }
        }
        true
    }

    /// 起一个后台批次。参数不齐时只给一条提示，面板留在编辑态。
    pub(super) fn start_convert(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.convert.as_ref() else {
            return;
        };
        if !dialog.editing() {
            return;
        }
        let target = dialog.target;
        // 起跑时把下拉收掉：批次开始后不该还挂着一个选择器。
        if let Some(dialog) = self.convert.as_mut() {
            dialog.picker.open = false;
            dialog.picker.query.clear();
        }

        let inputs = self.convert_inputs();
        if inputs.is_empty() {
            let message = self.convert_empty_reason();
            self.flash(message);
            cx.notify();
            return;
        }

        let requests = convert::plan(&inputs, target);
        let total = requests.len();
        let first = requests[0].input.clone();
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let job = match ConvertJob::start(requests, tx) {
            Ok(job) => job,
            Err(error) => {
                if let Some(dialog) = self.convert.as_mut() {
                    dialog.state = ConvertState::Failed(format!("{error:#}"));
                }
                cx.notify();
                return;
            }
        };

        // 每批任务领一个新代次：上一次取消后迟到的收尾事件不会再改写这一批的状态。
        self.convert_generation += 1;
        let generation = self.convert_generation;
        if let Some(dialog) = self.convert.as_mut() {
            dialog.generation = generation;
            dialog.job = Some(Arc::new(job));
            dialog.state = ConvertState::Running {
                index: 0,
                total,
                name: file_name(&first),
                ratio: None,
            };
        }
        cx.notify();

        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let alive = this.update(cx, |this, cx| {
                    let Some(dialog) = this.convert.as_mut() else {
                        return;
                    };
                    if dialog.generation != generation {
                        return;
                    }
                    match event {
                        ConvertEvent::ItemStarted { index, input } => {
                            if let ConvertState::Running {
                                index: current,
                                name,
                                ratio,
                                ..
                            } = &mut dialog.state
                            {
                                *current = index;
                                *name = file_name(&input);
                                *ratio = None;
                            }
                        }
                        ConvertEvent::Progress { index, ratio } => {
                            if let ConvertState::Running {
                                index: current,
                                ratio: slot,
                                ..
                            } = &mut dialog.state
                                && *current == index
                            {
                                *slot = ratio;
                            }
                        }
                        // 单项结果在收尾的汇总里一并给出，这里只留日志。
                        ConvertEvent::ItemFinished { .. } => {}
                        ConvertEvent::ItemFailed { input, message, .. } => {
                            log_warn!("格式转换失败: {} - {message}", input.display());
                        }
                        ConvertEvent::Finished(outcome) => {
                            dialog.job = None;
                            let (summary, cancelled) = match outcome {
                                ConvertOutcome::Done(summary) => (summary, false),
                                ConvertOutcome::Cancelled(summary) => (summary, true),
                            };
                            dialog.state = ConvertState::Done { summary, cancelled };
                        }
                    }
                    cx.notify();
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// 取消批次，面板回到编辑态（已经转好的文件留在原地）。
    pub(super) fn cancel_convert(&mut self, cx: &mut Context<Self>) {
        self.convert_generation += 1;
        let generation = self.convert_generation;
        if let Some(dialog) = self.convert.as_mut() {
            if let Some(job) = dialog.job.take() {
                job.cancel();
            }
            dialog.generation = generation;
            dialog.state = ConvertState::Editing;
        }
        cx.notify();
    }

    /// 开始按钮点不动的原因。文案本身就是提示，界面不用再拼一次。
    fn convert_empty_reason(&self) -> &'static str {
        match self
            .convert
            .as_ref()
            .map(|dialog| (&dialog.source, &dialog.scan))
        {
            Some((ConvertSource::Current, _)) => "没有正在播放的文件，选一个文件夹再试",
            Some((_, ScanState::Scanning)) => "还在扫描文件夹，读完再试",
            Some((_, ScanState::Failed(_))) => "文件夹读取失败，重新选一次",
            _ => "这里没有能转换的媒体文件",
        }
    }

    fn convert_first_output(&self) -> Option<PathBuf> {
        match &self.convert.as_ref()?.state {
            ConvertState::Done { summary, .. } => summary.outputs.first().cloned(),
            _ => None,
        }
    }

    pub(super) fn convert_open_result(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.convert_first_output() {
            match platform::open::open(&path) {
                Ok(()) => self.flash(format!("已打开 {}", path.display())),
                Err(message) => self.flash(message),
            }
        }
        cx.notify();
    }

    pub(super) fn convert_reveal_result(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.convert_first_output() {
            match platform::reveal::reveal(&path) {
                Ok(()) => self.flash("已在文件管理器里定位"),
                Err(message) => self.flash(message),
            }
        }
        cx.notify();
    }
}
