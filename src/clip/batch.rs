//! 批处理：把同一套导出参数套到一整批文件上，逐个跑、逐个回报。
//!
//! 每个文件仍然交给 [`ClipJob`] 走一遍完整流程——编码器选择、关键帧对齐、两遍响度、
//! 按大小切分、取消时清半成品，全都能原样复用。这里只负责四件事：
//!
//! 1. 先探出每个文件的时长：它既是进度分母，也是「整段」这个区间的终点；
//! 2. 串行排队：同时开几个编码器只会让每一项都变慢；
//! 3. 把单项进度折算成整批进度；
//! 4. 取消时把当前这一项也停掉，剩下的不再启动。

use super::encoder;
use super::job::ClipJob;
use super::naming::batch_output_path;
use super::{ClipEvent, ClipOutcome, ClipRange, ClipRequest, CutKind, ExportOptions, OutputFormat};
use anyhow::{Result, anyhow};
use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use futures::executor::block_on;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 这一批里的一项：输入与落点。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchItem {
    pub input: PathBuf,
    pub output: PathBuf,
}

/// 给一批输入配上落点，顺序保持不变。
///
/// 同名的两个文件（`a.mkv` 与 `a.mp4`）会算出同一个落点，这里一次排开编号，
/// 免得后一个把前一个的产物当「已存在」而不断往后跳。
pub fn plan_batch(inputs: &[PathBuf], format: OutputFormat) -> Vec<BatchItem> {
    let mut taken = HashSet::new();
    inputs
        .iter()
        .map(|input| {
            let output = batch_output_path(input, format, &taken);
            taken.insert(output.clone());
            BatchItem {
                input: input.clone(),
                output,
            }
        })
        .collect()
}

/// 后台任务回报给界面的事件。
#[derive(Clone, Debug)]
pub enum BatchEvent {
    /// 开始处理第 `index` 项。
    ItemStarted { index: usize, input: PathBuf },
    /// 当前项进度，0.0–1.0。
    Progress { index: usize, ratio: f32 },
    /// 一项收尾。`ok` 为假表示这一项失败了，界面用它累计「已完成 / 失败」。
    ItemFinished { index: usize, ok: bool },
    /// 整批收尾，之后不会再有事件。
    Finished(BatchOutcome),
}

/// 一项失败的原因，界面只挑前几条显示。
#[derive(Clone, Debug)]
pub struct BatchFailure {
    pub input: PathBuf,
    pub message: String,
}

/// 一批跑完后的账目。
#[derive(Clone, Debug, Default)]
pub struct BatchSummary {
    /// 产出的文件，按处理顺序；分割格式一项会有多个。
    pub outputs: Vec<PathBuf>,
    pub failures: Vec<BatchFailure>,
    /// 取消后没来得及处理的项数，含当时正在跑的那一项。
    pub skipped: usize,
}

#[derive(Clone, Debug)]
pub enum BatchOutcome {
    Done(BatchSummary),
    Cancelled(BatchSummary),
}

/// 正在跑的一批。界面只持有句柄：进度走事件通道，取消直接杀当前项。
pub struct BatchJob {
    /// 当前这一项的句柄，取消时要能立刻杀掉它。
    current: Arc<Mutex<Option<Arc<ClipJob>>>>,
    cancelled: Arc<AtomicBool>,
}

impl BatchJob {
    /// 启动后台批处理。编码器探测或 ffmpeg 缺失会在第一个文件之前就报错，
    /// 而不是让每一项各失败一遍。
    pub fn start(
        items: Vec<BatchItem>,
        options: ExportOptions,
        tx: UnboundedSender<BatchEvent>,
    ) -> Result<Self> {
        if items.is_empty() {
            return Err(anyhow!("没有需要处理的文件"));
        }
        let executable = crate::platform::ffmpeg::executable()?;
        // 强制硬件却探测不到可用编码器时，提前把原因说清楚。
        encoder::pick(options.codec, options.encoder_mode).map_err(anyhow::Error::msg)?;

        let current: Arc<Mutex<Option<Arc<ClipJob>>>> = Arc::new(Mutex::new(None));
        let cancelled = Arc::new(AtomicBool::new(false));
        let task_current = current.clone();
        let task_cancelled = cancelled.clone();

        std::thread::spawn(move || {
            let total = items.len();
            let mut summary = BatchSummary::default();
            for (index, item) in items.iter().enumerate() {
                if task_cancelled.load(Ordering::SeqCst) {
                    summary.skipped = total - index;
                    break;
                }
                let _ = tx.unbounded_send(BatchEvent::ItemStarted {
                    index,
                    input: item.input.clone(),
                });
                let result = run_one(
                    &executable,
                    item,
                    index,
                    &options,
                    &task_current,
                    &task_cancelled,
                    &tx,
                );
                match result {
                    ItemResult::Finished(outputs) => {
                        summary.outputs.extend(outputs);
                        let _ = tx.unbounded_send(BatchEvent::ItemFinished { index, ok: true });
                    }
                    ItemResult::Cancelled => {
                        summary.skipped = total - index;
                        break;
                    }
                    ItemResult::Failed(message) => {
                        summary.failures.push(BatchFailure {
                            input: item.input.clone(),
                            message,
                        });
                        let _ = tx.unbounded_send(BatchEvent::ItemFinished { index, ok: false });
                    }
                }
            }
            let outcome = if task_cancelled.load(Ordering::SeqCst) {
                BatchOutcome::Cancelled(summary)
            } else {
                BatchOutcome::Done(summary)
            };
            let _ = tx.unbounded_send(BatchEvent::Finished(outcome));
        });

        Ok(Self { current, cancelled })
    }

    /// 取消整批。正在跑的那一项会被杀掉，任务线程负责删掉半成品并回报。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(job) = self.current.lock().unwrap().as_ref() {
            job.cancel();
        }
    }
}

impl Drop for BatchJob {
    fn drop(&mut self) {
        // 关面板、关窗口时不能把 ffmpeg 留在后台继续写。
        self.cancel();
    }
}

enum ItemResult {
    Finished(Vec<PathBuf>),
    Cancelled,
    Failed(String),
}

/// 处理一项：先探时长，再借 `ClipJob` 跑一次整段导出，把它的进度转成整批的事件。
fn run_one(
    executable: &std::path::Path,
    item: &BatchItem,
    index: usize,
    options: &ExportOptions,
    current: &Arc<Mutex<Option<Arc<ClipJob>>>>,
    cancelled: &Arc<AtomicBool>,
    tx: &UnboundedSender<BatchEvent>,
) -> ItemResult {
    // 时长探不出来就做不了整段导出：`-t` 没有取值，写出来的文件是空的。
    let duration = match crate::platform::ffmpeg::probe_duration(executable, &item.input) {
        Ok(duration) if duration > 0.0 => duration,
        Ok(_) => return ItemResult::Failed("读不出这个文件的时长，已跳过".to_string()),
        Err(error) => return ItemResult::Failed(format!("{error:#}")),
    };

    let request = ClipRequest {
        input: item.input.clone(),
        output: item.output.clone(),
        // 整段：区间就是整个文件。起点卡在 0 上时 `ClipJob` 不会再去探关键帧。
        range: ClipRange {
            start: 0.0,
            end: duration,
        },
        options: options.clone(),
        // 每个文件的内置字幕轨不一样，批处理不做烧录。
        burn: None,
        loudness: None,
        kind: CutKind::Keep,
        duration,
    };

    let (inner_tx, mut inner_rx) = unbounded();
    let job = match ClipJob::start(request, inner_tx) {
        Ok(job) => Arc::new(job),
        Err(error) => return ItemResult::Failed(format!("{error:#}")),
    };
    // 取消恰好发生在这个文件刚起跑的那一刻：这里的补刀保证它立刻停下，
    // 而不是先白跑完一整份。
    if cancelled.load(Ordering::SeqCst) {
        job.cancel();
    }
    *current.lock().unwrap() = Some(job);

    // 同步等这一项跑完：整批本来就是串行的，另起线程只会让取消更难写。
    while let Some(event) = block_on(inner_rx.next()) {
        match event {
            ClipEvent::Progress(ratio) => {
                let _ = tx.unbounded_send(BatchEvent::Progress { index, ratio });
            }
            ClipEvent::Done(outcome) => {
                *current.lock().unwrap() = None;
                return match outcome {
                    ClipOutcome::Finished(paths) if !paths.is_empty() => {
                        ItemResult::Finished(paths)
                    }
                    ClipOutcome::Finished(_) => ItemResult::Failed("没有产出文件".to_string()),
                    ClipOutcome::Cancelled => ItemResult::Cancelled,
                    ClipOutcome::Failed(message) => ItemResult::Failed(message),
                };
            }
        }
    }
    *current.lock().unwrap() = None;
    ItemResult::Failed("任务线程没有回报结果就退出了".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 落点按批内顺序编号() {
        let dir = std::env::temp_dir().join(format!("ngy-batch-plan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let inputs = vec![dir.join("甲.mkv"), dir.join("乙.mkv")];
        let plan = plan_batch(&inputs, OutputFormat::Mkv);
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].output, dir.join("甲 - 批处理.mkv"));
        assert_eq!(plan[1].output, dir.join("乙 - 批处理.mkv"));
        assert_eq!(plan[0].input, inputs[0]);

        // 空输入不会 panic，也不会凭空造出落点。
        assert!(plan_batch(&[], OutputFormat::Mkv).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
