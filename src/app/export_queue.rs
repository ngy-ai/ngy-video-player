//! 导出队列：一次只跑一条，失败不阻塞后面。
//!
//! 导出按钮在任务跑着的时候不再「拒绝服务」，而是把新任务排到队尾——连着导几条
//! 是剪辑收尾时最常见的动作。队列只是一串待跑的 [`QueuedExport`]，真正干活仍然
//! 只有 `ExportTask` 一个，所以「同时只跑一条」是结构上就成立的。
//!
//! 工程目前只有一条序列（`Project::sequence`），所以队列装的是「同一条序列的多次
//! 导出」，而不是「多条序列各导一次」。等模型支持多序列时，这里换成按序列排队即可。

use std::path::PathBuf;

use super::*;
use crate::timeline::{ExportPlan, Tick};

/// 排队中的一次导出。
///
/// 计划在入队时就算好并定死：入队之后的编辑不该改写已经排上的活——用户点导出时
/// 认可的是当时那条片子。
#[derive(Clone, Debug)]
pub(super) struct QueuedExport {
    plan: ExportPlan,
    path: PathBuf,
    total: Tick,
    /// 这条用的预设名；`None` 表示参数被微调过或没存成预设。
    preset: Option<String>,
}

impl PlayerApp {
    /// 把一次导出排进队列；当前空闲就立刻开跑。
    pub(super) fn enqueue_export(
        &mut self,
        plan: ExportPlan,
        path: PathBuf,
        total: Tick,
        preset: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let label = preset_label(preset.as_deref());
        self.export_queue.push_back(QueuedExport {
            plan,
            path,
            total,
            preset,
        });
        if self.export_task.is_none() {
            self.start_next_export(cx);
            return;
        }
        self.flash(format!(
            "已加入导出队列（{label}，前面还有 {} 条）",
            self.export_queue.len()
        ));
        cx.notify();
    }

    /// 取队首开跑；队列空了就把进度收干净。
    pub(super) fn start_next_export(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self.export_queue.pop_front() else {
            self.export_task = None;
            self.export_progress = None;
            self.export_preset = None;
            cx.notify();
            return;
        };
        self.start_export(job.plan, job.path, job.total, job.preset, cx);
    }

    /// 标题条上的队列提示：还在排队的条数（不含正在跑的那条）。
    pub(super) fn export_queue_note(&self) -> Option<String> {
        queue_note(self.export_queue.len())
    }
}

/// 队列提示文案。空队列不显示——标题条已经够挤了，多一个「队列 0」只是噪音。
fn queue_note(pending: usize) -> Option<String> {
    (pending > 0).then(|| format!("队列 {pending}"))
}

/// 一条导出用的是哪套参数。提示里总要说清楚，不然队列连着几条时分不出谁是谁。
fn preset_label(preset: Option<&str>) -> String {
    match preset {
        Some(name) => format!("预设「{name}」"),
        None => "自定义参数".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 空队列不显示提示() {
        assert_eq!(queue_note(0), None);
        assert_eq!(queue_note(1).as_deref(), Some("队列 1"));
        assert_eq!(queue_note(3).as_deref(), Some("队列 3"));
    }

    #[test]
    fn 预设名与自定义分开说() {
        assert_eq!(preset_label(Some("交付版")), "预设「交付版」");
        assert_eq!(preset_label(None), "自定义参数");
    }
}
