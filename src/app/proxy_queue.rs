//! 批量补代理：把「比序列大、还没有代理」的素材串起来跑。
//!
//! 一次只跑一个：代理是 CPU 密集的活，两路 ffmpeg 同时跑只会让两边都慢一倍，
//! 时间线还跟着卡。队列不弹模态——进度贴在素材库底栏，用户可以接着剪。
//!
//! 只补「没有代理」的，不覆盖已有的：想换档重生成走单个素材那条对话框。

use super::*;
use crate::proxy::{self as proxy_core, ProxyEvent, ProxyJob, ProxyOutcome, ProxyQuality};
use std::collections::VecDeque;

/// 排队等着跑的一个素材。
pub(crate) struct ProxyTask {
    pub(crate) name: String,
    pub(crate) input: PathBuf,
    /// 进度条的分母（素材时长，秒）。
    pub(crate) duration: f64,
}

/// 正在跑的那一个。
pub(crate) struct ProxyRunning {
    pub(crate) name: String,
    pub(crate) job: Arc<ProxyJob>,
    pub(crate) progress: f32,
}

/// 批量补代理的队列。
pub(crate) struct ProxyQueue {
    pub(crate) pending: VecDeque<ProxyTask>,
    pub(crate) running: Option<ProxyRunning>,
    /// 整队共用一档规格：同一个工程里的代理不该五花八门。
    pub(crate) quality: ProxyQuality,
    pub(crate) total: usize,
    /// 已经跑完的条数（成功 + 失败），进度条用。
    pub(crate) done: usize,
    /// 失败的素材名与原因，收尾时一起报。
    pub(crate) failed: Vec<(String, String)>,
}

impl PlayerApp {
    pub(crate) fn proxy_queue_busy(&self) -> bool {
        self.proxy_queue.is_some()
    }

    /// 把素材库里「比序列大、还没有代理」的素材排进队列并开跑。
    ///
    /// 返回排进去的条数：0 表示没活可干（调用方据此决定要不要提示）。
    pub(crate) fn queue_missing_proxies(
        &mut self,
        quality: ProxyQuality,
        cx: &mut Context<Self>,
    ) -> usize {
        if self.proxy_queue.is_some() {
            return 0;
        }
        let tasks = {
            let Some(project) = self.project.as_ref() else {
                return 0;
            };
            project
                .assets
                .iter()
                .filter(|asset| crate::timeline::needs_proxy(project, asset.id))
                .filter_map(|asset| {
                    let media = asset.media.as_ref()?;
                    Some(ProxyTask {
                        name: asset.display_name(),
                        input: asset.resolved.clone(),
                        duration: media.duration.secs(),
                    })
                })
                .collect::<VecDeque<_>>()
        };
        if tasks.is_empty() {
            return 0;
        }
        let total = tasks.len();
        self.proxy_queue_generation += 1;
        self.proxy_queue = Some(ProxyQueue {
            pending: tasks,
            running: None,
            quality,
            total,
            done: 0,
            failed: Vec::new(),
        });
        self.start_next_proxy(cx);
        total
    }

    /// 跳过正在跑的那一个：杀掉进程、删半成品，队列接着跑下一个。
    ///
    /// 与 [`Self::cancel_proxy_queue`] 的区别：那个是整队作废，这个只是这一条不做了。
    pub(crate) fn skip_current_proxy(&mut self, cx: &mut Context<Self>) {
        let Some(running) = self
            .proxy_queue
            .as_mut()
            .and_then(|queue| queue.running.take())
        else {
            return;
        };
        running.job.cancel();
        if let Some(queue) = self.proxy_queue.as_mut() {
            queue.done += 1;
            queue.failed.push((running.name, "已跳过".to_string()));
        }
        // 代次一变，这条迟到的完成事件就碰不到队列了（否则会把下一条的状态顶掉）。
        self.proxy_queue_generation += 1;
        self.start_next_proxy(cx);
        cx.notify();
    }

    /// 取消整队：正在跑的那个杀掉并删半成品，排队的直接丢掉。
    pub(crate) fn cancel_proxy_queue(&mut self, cx: &mut Context<Self>) {
        let Some(queue) = self.proxy_queue.take() else {
            return;
        };
        if let Some(running) = queue.running {
            running.job.cancel();
        }
        // 代次一变，还在路上的事件就都作废了。
        self.proxy_queue_generation += 1;
        self.flash("已取消补代理：半成品会删掉，已经生成的留着");
        cx.notify();
    }

    /// 起下一个；队列空了就收尾。
    fn start_next_proxy(&mut self, cx: &mut Context<Self>) {
        let Some(queue) = self.proxy_queue.as_mut() else {
            return;
        };
        let Some(task) = queue.pending.pop_front() else {
            let Some(queue) = self.proxy_queue.take() else {
                return;
            };
            self.finish_proxy_queue(queue);
            cx.notify();
            return;
        };
        let generation = self.proxy_queue_generation;
        let request = proxy_core::ProxyRequest {
            input: task.input.clone(),
            output: proxy_core::output_path(&task.input),
            duration: task.duration,
            quality: queue.quality,
            // 队列没有逐条的参数面板，码率一律用档位自带的；要自定义就单条走对话框。
            max_bitrate: queue.quality.max_bitrate(),
        };
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        match ProxyJob::start(request, tx) {
            Ok(job) => {
                if let Some(queue) = self.proxy_queue.as_mut() {
                    queue.running = Some(ProxyRunning {
                        name: task.name,
                        job: Arc::new(job),
                        progress: 0.0,
                    });
                }
            }
            Err(error) => {
                // ffmpeg 都起不来：这一条记失败，接着下一条，别让整队卡死。
                if let Some(queue) = self.proxy_queue.as_mut() {
                    queue.done += 1;
                    queue.failed.push((task.name, format!("{error:#}")));
                }
                self.start_next_proxy(cx);
                return;
            }
        }

        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let alive = this.update(cx, |this, cx| {
                    if this.proxy_queue_generation != generation {
                        return;
                    }
                    match event {
                        ProxyEvent::Progress(progress) => {
                            if let Some(queue) = this.proxy_queue.as_mut()
                                && let Some(running) = queue.running.as_mut()
                            {
                                running.progress = progress;
                            }
                            cx.notify();
                        }
                        ProxyEvent::Done(outcome) => {
                            let Some(queue) = this.proxy_queue.as_mut() else {
                                return;
                            };
                            let name = queue
                                .running
                                .take()
                                .map(|running| running.name)
                                .unwrap_or_default();
                            queue.done += 1;
                            if let ProxyOutcome::Failed(message) = outcome {
                                queue.failed.push((name, message));
                            }
                            this.start_next_proxy(cx);
                            cx.notify();
                        }
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn finish_proxy_queue(&mut self, queue: ProxyQueue) {
        // 跑完一轮，缺代理的名单变了：按钮上的数字要跟着更新。
        self.refresh_library_states();
        let failed = queue.failed.len();
        let ok = queue.total.saturating_sub(failed);
        let message = if failed == 0 {
            format!("补代理完成：{ok} 个")
        } else if let Some((name, reason)) = queue.failed.first() {
            format!("补代理完成：{ok} 个成功，{failed} 个失败（{name}：{reason}）")
        } else {
            format!("补代理完成：{ok} 个成功")
        };
        self.flash(message);
    }
}
