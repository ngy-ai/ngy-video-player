//! A-B 循环：只反复播放标记出来的一小段，适合听一句台词、看一个动作。
//!
//! 不用 mpv 的 `ab-loop-a/b` 选项：那对选项改的是引擎内部状态，界面拿不到
//! 「现在标记到哪」的回读，暂停、换片、清空时容易和界面显示对不上。这里由界面
//! 自己记两个时间点，播放位置越过 B 就 seek 回 A——位置事件本来就是每帧都在收，
//! 判断一次的成本可以忽略。

use super::*;

impl PlayerApp {
    /// 标记 A 点。重新标记时 B 点一起清掉：旧的终点多半已经不在新区间里。
    pub(super) fn mark_ab_start(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.snapshot.path.clone() else {
            self.flash("没有正在播放的文件");
            cx.notify();
            return;
        };
        let position = self.snapshot.position.max(0.0);
        self.ab_loop = AbLoop {
            path: Some(path),
            start: Some(position),
            end: None,
            seeked_at: None,
        };
        self.flash(format!("A 点：{}", format_time(position)));
        cx.notify();
    }

    /// 标记 B 点。必须晚于 A 点，否则循环区间是负的。
    pub(super) fn mark_ab_end(&mut self, cx: &mut Context<Self>) {
        let Some(start) = self.ab_loop.start else {
            self.flash("先标记 A 点，再标记 B 点");
            cx.notify();
            return;
        };
        let position = self.snapshot.position.max(0.0);
        if position <= start + 0.1 {
            self.flash("B 点必须晚于 A 点");
            cx.notify();
            return;
        }
        self.ab_loop.end = Some(position);
        // 标记完立刻回到 A 点：接下来按下播放就是从区间开头开始。
        self.ab_loop.seeked_at = Some(Instant::now());
        self.send(Command::SeekAbsolute(start));
        self.flash(format!(
            "A-B 循环：{} → {}",
            format_time(start),
            format_time(position)
        ));
        cx.notify();
    }

    pub(super) fn clear_ab_loop(&mut self, cx: &mut Context<Self>) {
        if self.ab_loop.start.is_none() {
            return;
        }
        self.ab_loop = AbLoop::default();
        self.flash("已清除 A-B 循环");
        cx.notify();
    }

    /// 播放位置越过 B 点时跳回 A 点。由 `render` 每帧调用。
    pub(super) fn poll_ab_loop(&mut self, cx: &mut Context<Self>) {
        // 工程模式下播放头归时间线管：A-B 回跳会把预览的素材拽回上一个位置，
        // 两个循环互相拉扯。标记本身留着，关掉工程后照常生效。
        if self.project.is_some() {
            return;
        }
        let (Some(path), Some(start), Some(end)) = (
            self.ab_loop.path.as_ref(),
            self.ab_loop.start,
            self.ab_loop.end,
        ) else {
            return;
        };
        // 换片后旧的标记没有意义：时长、位置都对不上，直接清掉。
        if self.snapshot.path.as_deref() != Some(path.as_path()) {
            self.ab_loop = AbLoop::default();
            return;
        }
        // 暂停时不回跳：用户正在逐帧看 B 点附近，被拽回 A 点会莫名其妙。
        if self.snapshot.paused || self.snapshot.idle {
            return;
        }
        // 刚发出 seek 时位置事件可能还是旧的，给引擎一点时间；否则会在
        // 「跳回 A」和「A 之后又越过 B」之间反复触发。
        if self
            .ab_loop
            .seeked_at
            .is_some_and(|at| at.elapsed() < Duration::from_millis(600))
        {
            return;
        }
        if self.snapshot.position < end - 0.03 {
            return;
        }
        self.ab_loop.seeked_at = Some(Instant::now());
        self.send(Command::SeekAbsolute(start));
        cx.notify();
    }
}
