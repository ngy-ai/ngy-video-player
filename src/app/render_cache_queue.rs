//! 渲染缓存队列（M10-T19）：把「算不动」的片段串起来预渲染。
//!
//! 与补代理同一套语义：一次只跑一条（两路 ffmpeg 同时跑只会让两边都慢一倍）、
//! 队列可以随时取消、进度贴在设置窗口「渲染缓存」那一页——不弹模态，用户可以
//! 接着剪。
//!
//! 与补代理的差别只在产物：代理是**素材**的整段低码率副本，缓存是**片段**加过
//! 效果的成片尺寸小文件。缓存只给预览用，导出永远重新算一遍。
//!
//! 队列里的一条就是一份「只渲这一段」的导出计划（[`crate::timeline::cache_plan`]），
//! 所以效果、调色、关键帧全走导出那条滤镜链——预览看到的就是成片会是什么样。

use super::*;
use crate::export::{ExportEvent, ExportTask};
use crate::timeline::{
    CacheEntry, DEFAULT_LIMIT_MB, MAX_LIMIT_MB, MIN_LIMIT_MB, Tick, cache_path, cache_plan,
    size_label, total_bytes, trim_plan, worth_caching,
};

use std::collections::VecDeque;

/// 排队等着渲染的一个片段。
pub(crate) struct CacheTask {
    /// 片段名（提示与失败清单里用）。
    pub(crate) name: String,
    pub(crate) output: PathBuf,
    pub(crate) plan: crate::timeline::ExportPlan,
    /// 进度条的分母。
    pub(crate) total: Tick,
}

/// 正在渲染的那一个。
pub(crate) struct CacheRunning {
    pub(crate) name: String,
    pub(crate) task: Arc<ExportTask>,
    pub(crate) progress: f32,
}

/// 渲染缓存队列。
pub(crate) struct RenderCacheQueue {
    pub(crate) pending: VecDeque<CacheTask>,
    pub(crate) running: Option<CacheRunning>,
    pub(crate) total: usize,
    pub(crate) done: usize,
    pub(crate) failed: Vec<(String, String)>,
}

impl PlayerApp {
    /// 缓存目录。跟着 `NGY_VIDEO_CACHE_DIR` 走，与缩略图、波形放一处。
    pub(crate) fn render_cache_dir(&self) -> PathBuf {
        crate::media_cache::cache_root().join(crate::timeline::CACHE_DIR_NAME)
    }

    /// 重新扫一遍缓存目录，把占用与条目刷进界面状态。
    ///
    /// 只在打开设置页、队列收尾、清理之后调用：每帧都 `read_dir` 会把拖时间线拖成幻灯片。
    pub(crate) fn refresh_cache_stats(&mut self) {
        let dir = self.render_cache_dir();
        let mut entries = Vec::new();
        if let Ok(list) = std::fs::read_dir(&dir) {
            for item in list.flatten() {
                let path = item.path();
                let is_cache = path
                    .file_name()
                    .map(|name| {
                        name.to_string_lossy()
                            .ends_with(crate::timeline::CACHE_SUFFIX)
                    })
                    .unwrap_or(false);
                if !is_cache {
                    continue;
                }
                let Ok(meta) = item.metadata() else {
                    continue;
                };
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|since| since.as_secs())
                    .unwrap_or(0);
                entries.push(CacheEntry {
                    path,
                    bytes: meta.len(),
                    modified,
                });
            }
        }
        self.cache_bytes = total_bytes(&entries);
        self.cache_entries = entries;
    }

    /// 当前序列里「该有缓存、但还没有」的片段数。按钮上的数字用它。
    pub(crate) fn missing_cache_count(&self) -> usize {
        let Some(project) = self.project.as_ref() else {
            return 0;
        };
        let sequence = project.sequence();
        let dir = self.render_cache_dir();
        sequence
            .tracks
            .iter()
            .flat_map(|track| track.clips.iter())
            .filter(|clip| worth_caching(clip))
            .filter(|clip| !cache_path(&dir, project, sequence, clip).exists())
            .count()
    }

    /// 把当前序列里缺缓存的片段排进队列并开跑。返回排进去的条数。
    pub(crate) fn queue_missing_caches(&mut self, cx: &mut Context<Self>) -> usize {
        if self.cache_queue.is_some() {
            return 0;
        }
        let dir = self.render_cache_dir();
        let tasks = {
            let Some(project) = self.project.as_ref() else {
                return 0;
            };
            let sequence = project.sequence();
            let mut tasks = VecDeque::new();
            for track in &sequence.tracks {
                for clip in &track.clips {
                    if !worth_caching(clip) {
                        continue;
                    }
                    let output = cache_path(&dir, project, sequence, clip);
                    if output.exists() {
                        continue;
                    }
                    let Ok(plan) = cache_plan(project, sequence, clip) else {
                        // 素材缺失之类：跳过这一条，别让整队排不出来。
                        continue;
                    };
                    tasks.push_back(CacheTask {
                        name: project
                            .asset(clip.asset_id)
                            .map(|asset| asset.display_name())
                            .unwrap_or_else(|| "片段".to_string()),
                        output,
                        plan,
                        total: clip.duration(),
                    });
                }
            }
            tasks
        };
        if tasks.is_empty() {
            return 0;
        }
        let total = tasks.len();
        self.cache_queue_generation += 1;
        self.cache_queue = Some(RenderCacheQueue {
            pending: tasks,
            running: None,
            total,
            done: 0,
            failed: Vec::new(),
        });
        self.start_next_cache(cx);
        total
    }

    /// 取消整队：正在跑的那个杀掉并删半成品，排队的直接丢掉。
    pub(crate) fn cancel_cache_queue(&mut self, cx: &mut Context<Self>) {
        let Some(queue) = self.cache_queue.take() else {
            return;
        };
        // 半成品由任务自己在取消时删掉（它知道输出路径）。
        if let Some(running) = queue.running {
            running.task.cancel();
        }
        self.cache_queue_generation += 1;
        self.flash("已取消补渲染缓存：半成品会删掉，已经生成的留着");
        cx.notify();
    }

    /// 清空缓存目录。返回删掉的文件数。
    pub(crate) fn clear_render_cache(&mut self, cx: &mut Context<Self>) -> usize {
        let dir = self.render_cache_dir();
        let mut removed = 0;
        if let Ok(list) = std::fs::read_dir(&dir) {
            for item in list.flatten() {
                let path = item.path();
                if path
                    .file_name()
                    .map(|name| {
                        name.to_string_lossy()
                            .ends_with(crate::timeline::CACHE_SUFFIX)
                    })
                    .unwrap_or(false)
                    && std::fs::remove_file(&path).is_ok()
                {
                    removed += 1;
                }
            }
        }
        self.refresh_cache_stats();
        self.flash(format!("已清理 {removed} 个渲染缓存文件"));
        cx.notify();
        removed
    }

    /// 目录超出上限时按最旧优先删。返回删掉的条数。
    pub(crate) fn trim_render_cache(&mut self) -> usize {
        self.refresh_cache_stats();
        let limit = self
            .settings
            .render_cache_limit_mb
            .clamp(MIN_LIMIT_MB, MAX_LIMIT_MB)
            * 1024
            * 1024;
        let victims = trim_plan(&self.cache_entries, limit);
        let mut removed = 0;
        for path in victims {
            if std::fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        if removed > 0 {
            self.refresh_cache_stats();
        }
        removed
    }

    /// 容量上限的文案（设置页用）。
    pub(crate) fn cache_limit_label(&self) -> String {
        let limit = self
            .settings
            .render_cache_limit_mb
            .clamp(MIN_LIMIT_MB, MAX_LIMIT_MB);
        if limit == DEFAULT_LIMIT_MB {
            format!("{}（默认）", size_label(limit * 1024 * 1024))
        } else {
            size_label(limit * 1024 * 1024)
        }
    }

    /// 起下一个；队列空了就收尾。
    fn start_next_cache(&mut self, cx: &mut Context<Self>) {
        let Some(queue) = self.cache_queue.as_mut() else {
            return;
        };
        let Some(task) = queue.pending.pop_front() else {
            let Some(queue) = self.cache_queue.take() else {
                return;
            };
            self.finish_cache_queue(queue, cx);
            return;
        };
        // 目录（可能还有上层的 NGY_VIDEO_CACHE_DIR）不一定存在：ffmpeg 不会替我们建。
        if let Some(parent) = task.output.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            if let Some(queue) = self.cache_queue.as_mut() {
                queue.done += 1;
                queue
                    .failed
                    .push((task.name, format!("建不了缓存目录：{error}")));
            }
            self.start_next_cache(cx);
            return;
        }

        let generation = self.cache_queue_generation;
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        match ExportTask::start(task.plan.clone(), task.output.clone(), task.total, tx) {
            Ok(task_handle) => {
                if let Some(queue) = self.cache_queue.as_mut() {
                    queue.running = Some(CacheRunning {
                        name: task.name,
                        task: Arc::new(task_handle),
                        progress: 0.0,
                    });
                }
            }
            Err(error) => {
                // ffmpeg 都起不来：记一笔失败，接着下一条。
                if let Some(queue) = self.cache_queue.as_mut() {
                    queue.done += 1;
                    queue.failed.push((task.name, format!("{error:#}")));
                }
                self.start_next_cache(cx);
                return;
            }
        }

        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let alive = this.update(cx, |this, cx| {
                    if this.cache_queue_generation != generation {
                        return;
                    }
                    match event {
                        ExportEvent::Progress(progress) => {
                            if let Some(queue) = this.cache_queue.as_mut()
                                && let Some(running) = queue.running.as_mut()
                            {
                                running.progress = progress;
                            }
                            cx.notify();
                        }
                        ExportEvent::Finished(_)
                        | ExportEvent::Failed(_)
                        | ExportEvent::Cancelled => {
                            let Some(queue) = this.cache_queue.as_mut() else {
                                return;
                            };
                            let name = queue
                                .running
                                .take()
                                .map(|running| running.name)
                                .unwrap_or_default();
                            queue.done += 1;
                            match event {
                                ExportEvent::Failed(message) => {
                                    queue.failed.push((name, message));
                                }
                                ExportEvent::Cancelled => {
                                    queue.failed.push((name, "已取消".to_string()));
                                }
                                _ => {}
                            }
                            this.start_next_cache(cx);
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

    fn finish_cache_queue(&mut self, queue: RenderCacheQueue, cx: &mut Context<Self>) {
        // 跑完一轮按上限收一遍：缓存是「算得出来的东西」，删了只是下次再算。
        let trimmed = self.trim_render_cache();
        let failed = queue.failed.len();
        let ok = queue.total.saturating_sub(failed);
        let mut message = format!("渲染缓存补完：{ok} 段");
        if failed > 0
            && let Some((name, reason)) = queue.failed.first()
        {
            message.push_str(&format!("，{failed} 段失败（{name}：{reason}）"));
        }
        if trimmed > 0 {
            message.push_str(&format!("；超出上限删掉 {trimmed} 个旧缓存"));
        }
        self.flash(message);
        cx.notify();
    }
}

/// 一组可选的容量档位（MB）。
pub(crate) const CACHE_LIMIT_CHOICES: [u64; 4] = [2048, 4096, 8192, 16_384];

/// 队列状态文案：`渲染中 3/7 · 42%`。
pub(crate) fn cache_progress_label(queue: &RenderCacheQueue) -> String {
    let percent = queue
        .running
        .as_ref()
        .map(|running| (running.progress * 100.0).round() as i32)
        .unwrap_or(0);
    let done = queue.done.min(queue.total);
    format!("渲染中 {done}/{} · {percent}%", queue.total)
}
