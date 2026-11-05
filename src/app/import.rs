//! 把媒体文件导入时间线：登记素材、探测、接进序列。
//!
//! 单个文件优先落在播放头，撞上了就退到末尾；整批导入（文件框多选、整页列表）
//! 则一条接一条接在序列末尾，免得和播放头附近的已有内容互相挤。
//!
//! 「导入整页」可能一次几十个文件，所以那条路把 `ffmpeg -i` 放到后台跑，
//! 结果收齐后主线程一次性插入。

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use crate::timeline::{AssetId, ClipSpec, EditOp, MediaInfo, TrackKind, apply as apply_edit};

/// 正在从播放列表导入的一批素材。
///
/// `generation` 用来认领后台结果：换工程、关工程之后这批结果才回来时，
/// 不该再往新工程里插片段。`cancelled` 让后台探测也能提前收手，
/// 不必为一份已经关掉的工程把整页文件读完。
pub(super) struct PendingImport {
    generation: u64,
    cancelled: Arc<AtomicBool>,
}

impl PlayerApp {
    /// 菜单「导入到时间线…」：选文件，插到播放头。
    pub(super) fn import_to_timeline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.is_none() || self.dialog_open {
            return;
        }
        self.dialog_open = true;

        let mut picker = rfd::FileDialog::new()
            .add_filter("视频", formats::VIDEO)
            .add_filter("音频", formats::AUDIO)
            .set_title("导入到时间线")
            .set_parent(window);
        if let Some(directory) = self.settings.last_dir.clone() {
            picker = picker.set_directory(directory);
        }

        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(picker.pick_files());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if let Some(paths) = picked {
                    this.import_paths(paths, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 菜单「从当前列表导入到时间线」。
    ///
    /// 直接拿当前页当输入，不再让用户挑一遍——列表里已经是排好的顺序了。
    /// 探测放到后台：整页可能有几十集，同步跑 ffmpeg 会把界面卡住。结果收齐后
    /// 一次性插进序列，中途不会留下「只插进去一半」的状态挡在撤销前面。
    pub(super) fn import_playlist_to_timeline(&mut self, cx: &mut Context<Self>) {
        if self.project.is_none() {
            return;
        }
        if self.pending_import.is_some() {
            self.flash("正在从播放列表导入，等这一批完成");
            cx.notify();
            return;
        }
        let paths = self.playlist().file_paths();
        if paths.is_empty() {
            self.flash("播放列表里还没有文件");
            cx.notify();
            return;
        }
        let total = paths.len();

        self.import_generation += 1;
        let generation = self.import_generation;
        let cancelled = Arc::new(AtomicBool::new(false));
        self.pending_import = Some(PendingImport {
            generation,
            cancelled: cancelled.clone(),
        });
        self.flash(format!("正在探测 {total} 个素材…"));
        cx.notify();

        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        std::thread::spawn(move || {
            for path in paths {
                // 换工程时不必把整页探测完：剩下的结果已经没人要了。
                if cancelled.load(Ordering::Relaxed) {
                    break;
                }
                let info = crate::media::probe(&path).map_err(|error| format!("{error:#}"));
                if tx.unbounded_send((path, info)).is_err() {
                    break;
                }
            }
        });

        cx.spawn(async move |this, cx| {
            let mut results = Vec::with_capacity(total);
            while let Some(item) = rx.next().await {
                results.push(item);
            }
            let _ = this.update(cx, |this, cx| {
                this.finish_playlist_import(generation, results);
                cx.notify();
            });
        })
        .detach();
    }

    /// 换工程 / 关工程：正在跑的这批导入不再有意义，结果回来时一律丢掉。
    pub(super) fn cancel_pending_import(&mut self) {
        if let Some(pending) = self.pending_import.take() {
            pending.cancelled.store(true, Ordering::Relaxed);
            // 自增代次，旧结果到达时认不出来，也不会改写新工程。
            self.import_generation += 1;
        }
    }

    /// 整页探测完毕：按列表顺序一条接一条接在序列末尾，整批算一步撤销。
    fn finish_playlist_import(
        &mut self,
        generation: u64,
        results: Vec<(PathBuf, Result<MediaInfo, String>)>,
    ) {
        let Some(pending) = self.pending_import.as_ref() else {
            return;
        };
        if pending.generation != generation {
            return;
        }
        self.pending_import = None;

        // 起点取处理时的末尾，而不是探测开始时的：等待期间用户可能又改了几处，
        // 新素材应该排在那些改动之后，而不是和它们抢位置。
        let mut cursor = self
            .project
            .as_ref()
            .map(|project| project.sequence.duration())
            .unwrap_or(Tick::ZERO);
        let mut imported = 0;
        let mut failed = 0;

        for (path, info) in results {
            let info = match info {
                Ok(info) => info,
                Err(message) => {
                    log_warn!("导入「{}」失败：{message}", path.display());
                    failed += 1;
                    continue;
                }
            };

            let Some(project) = self.project.as_mut() else {
                break;
            };
            let id = match crate::media::register_probed(project, &path, info) {
                Ok(id) => id,
                Err(error) => {
                    log_warn!("导入「{}」失败：{error:#}", path.display());
                    failed += 1;
                    continue;
                }
            };
            // 借用不能跨过 `insert_probed`：先把时长与轨道信息拿出来。
            let Some(media) = self
                .project
                .as_ref()
                .and_then(|project| project.asset(id))
                .and_then(|asset| asset.media.clone())
            else {
                failed += 1;
                continue;
            };
            if media.duration <= Tick::ZERO {
                log_warn!("「{}」读不出时长，跳过", path.display());
                failed += 1;
                continue;
            }

            match self.insert_probed(id, &media, cursor) {
                Some(end) => {
                    cursor = end;
                    imported += 1;
                }
                None => failed += 1,
            }
        }

        if imported > 0 {
            self.remember_history();
        }
        self.report_import(imported, failed);
        self.poke();
    }

    /// 文件选择框那一路：同步探测，逐个插进时间线。
    ///
    /// 多选时整批接在序列末尾；只选一个文件时仍优先落在播放头，撞上了再退到末尾。
    fn import_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let batch = paths.len() > 1;
        let mut cursor = self
            .project
            .as_ref()
            .map(|project| project.sequence.duration());
        let mut imported = 0;
        let mut failed = 0;

        for path in &paths {
            let candidates: Vec<Tick> = if batch {
                cursor.into_iter().collect()
            } else {
                let playhead = self
                    .project
                    .as_ref()
                    .map(|project| project.ui.playhead)
                    .unwrap_or(Tick::ZERO);
                let end = self
                    .project
                    .as_ref()
                    .map(|project| project.sequence.duration())
                    .unwrap_or(Tick::ZERO);
                vec![playhead, end]
            };
            if candidates.is_empty() {
                failed += 1;
                continue;
            }
            match self.insert_media(path, &candidates) {
                Some(end) => {
                    cursor = Some(end);
                    imported += 1;
                }
                None => failed += 1,
            }
        }

        // 整批算一步撤销：按一次「导入」却要按 N 次撤销才能退回去太别扭。
        if imported > 0 {
            self.remember_history();
        }
        self.report_import(imported, failed);
        self.poke();
        cx.notify();
    }

    /// 登记并插入一个文件。`candidates` 是按顺序尝试的起点，第一个放得下就用。
    ///
    /// 返回插入后的终点；没插进去返回 `None`。
    fn insert_media(&mut self, path: &Path, candidates: &[Tick]) -> Option<Tick> {
        let project = self.project.as_mut()?;
        let id = match crate::media::register(project, path) {
            Ok(id) => id,
            Err(error) => {
                log_warn!("导入「{}」失败：{error:#}", path.display());
                return None;
            }
        };
        // 先拿到素材信息再改序列：`asset` 的借用不能跨过 `sequence` 的可变借用。
        let media = self
            .project
            .as_ref()?
            .asset(id)
            .and_then(|asset| asset.media.clone())?;
        if media.duration <= Tick::ZERO {
            log_warn!("「{}」读不出时长，跳过", path.display());
            return None;
        }

        candidates
            .iter()
            .find_map(|at| self.insert_probed(id, &media, *at))
    }

    /// 把探测好的素材插到 `at`。所有目标轨道都放得下才算成功。
    ///
    /// 视频与音频用同一个起点：分成两次各找位置会让声音和画面对不齐。
    fn insert_probed(&mut self, id: AssetId, media: &MediaInfo, at: Tick) -> Option<Tick> {
        let project = self.project.as_mut()?;
        let targets = target_tracks(&project.sequence, media.has_video(), media.has_audio());
        if targets.is_empty() {
            return None;
        }

        // 先在副本上排完，全部成功才写回：插一半失败会留下没有声音的画面。
        let mut next = project.sequence.clone();
        for track in targets {
            let spec = ClipSpec::new(id, Tick::ZERO, media.duration);
            match apply_edit(
                &next,
                &EditOp::Insert {
                    track,
                    at,
                    clip: spec,
                },
            ) {
                Ok(sequence) => next = sequence,
                Err(error) => {
                    let name = project
                        .asset_path(id)
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| format!("素材 {id:?}"));
                    log_warn!("「{name}」放不进时间线：{error}");
                    return None;
                }
            }
        }

        project.sequence = next;
        Some(at + media.duration)
    }

    fn report_import(&mut self, imported: usize, failed: usize) {
        if imported > 0 {
            self.flash(format!("已导入 {imported} 个素材"));
        }
        if failed > 0 {
            self.error = Some(Banner::new(
                "有素材没有导入",
                "这些文件可能不是视频/音频、已经损坏，或者没有可用的轨道。详细原因见日志。",
            ));
        }
    }
}

/// 一个素材该落到哪些轨道：有画面进视频轨、有声音进音频轨，锁定的轨道跳过。
fn target_tracks(
    sequence: &crate::timeline::Sequence,
    has_video: bool,
    has_audio: bool,
) -> Vec<crate::timeline::Id> {
    sequence
        .tracks
        .iter()
        .filter(|track| !track.locked)
        .filter(|track| match track.kind {
            TrackKind::Video => has_video,
            TrackKind::Audio => has_audio,
            TrackKind::Subtitle => false,
        })
        .map(|track| track.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::{Rational, Sequence};

    #[test]
    fn 素材按有无画面与声音分配到轨道() {
        let sequence = Sequence::new("测试", Rational::FPS_25, 1920, 1080);
        let video = sequence.tracks[0].id;
        let audio = sequence.tracks[1].id;

        assert_eq!(target_tracks(&sequence, true, true), vec![video, audio]);
        assert_eq!(target_tracks(&sequence, true, false), vec![video]);
        assert_eq!(target_tracks(&sequence, false, true), vec![audio]);
        assert!(target_tracks(&sequence, false, false).is_empty());
    }

    #[test]
    fn 锁定的轨道不收素材() {
        let mut sequence = Sequence::new("测试", Rational::FPS_25, 1920, 1080);
        let video = sequence.tracks[0].id;
        sequence.track_mut(video).unwrap().locked = true;
        assert!(target_tracks(&sequence, true, false).is_empty());
    }
}
