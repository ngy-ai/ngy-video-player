//! 与 OpenTimelineIO 换手（M10-T17）：导出 `.otio`、从 `.otio` 建一条新序列。
//!
//! 转换本身在 `timeline::otio`（纯数据、可单测），这里只做三件界面该做的事：
//! 选文件、把素材登记进工程、把结果变成一条能切换过去的序列。
//!
//! 导入**新建**一条序列而不是覆盖当前那条：对方给的 OTIO 里轨道顺序、规格都可能
//! 跟手上这条不一样，覆盖等于把正在剪的东西抹掉。新建一条之后不想用，删掉即可。

use super::*;
use crate::timeline::otio::{self, ImportedTimeline};
use crate::timeline::{
    AssetId, ClipItem, Id, Sequence, SequenceSpec, SubtitleItem, Tick, TrackKind,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

impl PlayerApp {
    /// 把当前序列导出成 `.otio`。
    pub(super) fn export_otio(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            self.flash("还没有打开工程");
            cx.notify();
            return;
        };
        let text = otio::export_sequence(project, project.sequence());
        let default_name = format!("{}.otio", project.sequence().name);
        if self.dialog_open {
            self.flash("已经有一个文件选择框开着，先把它处理掉");
            cx.notify();
            return;
        }

        let directory = self
            .project_path
            .as_ref()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .or_else(|| self.settings.last_dir.clone());
        self.dialog_open = true;
        let mut picker = rfd::FileDialog::new()
            .add_filter("OTIO 时间线", &[otio::OTIO_EXTENSION])
            .set_file_name(&default_name)
            .set_parent(window);
        if let Some(directory) = directory {
            picker = picker.set_directory(directory);
        }
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(picker.save_file());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if let Some(path) = picked {
                    this.remember_dir(&path);
                    match std::fs::write(&path, text) {
                        Ok(()) => this.flash(format!("已导出 OTIO：{}", path.display())),
                        Err(error) => {
                            this.error = Some(Banner::new(
                                "导出 OTIO 失败",
                                format!("{}：{error}", path.display()),
                            ));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 从一份 `.otio` 建一条新序列并切过去。
    pub(super) fn import_otio(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project.is_none() {
            self.flash("先新建或打开一个工程，再导入 OTIO");
            cx.notify();
            return;
        }
        if self.dialog_open {
            self.flash("已经有一个文件选择框开着，先把它处理掉");
            cx.notify();
            return;
        }

        self.dialog_open = true;
        let mut picker = rfd::FileDialog::new()
            .add_filter("OTIO 时间线", &[otio::OTIO_EXTENSION])
            .set_parent(window);
        if let Some(directory) = self.settings.last_dir.clone() {
            picker = picker.set_directory(directory);
        }
        let (tx, rx) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(picker.pick_file());
        });

        cx.spawn(async move |this, cx| {
            let picked = rx.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.dialog_open = false;
                if let Some(path) = picked {
                    this.remember_dir(&path);
                    this.apply_otio(&path, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// 读文件、登记素材、建序列。
    fn apply_otio(&mut self, path: &Path, cx: &mut Context<Self>) {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                self.error = Some(Banner::new(
                    "导入 OTIO 失败",
                    format!("读不了 {}：{error}", path.display()),
                ));
                return;
            }
        };
        let imported = match otio::import_timeline(&text) {
            Ok(imported) => imported,
            Err(error) => {
                self.error = Some(Banner::new("导入 OTIO 失败", error.to_string()));
                return;
            }
        };

        let spec = self.otio_spec(&imported);
        let name = imported.name.clone();
        let paths = otio_paths(&imported);
        // 素材先全部登记：`add_asset` 只写素材表，缺的探测随后由素材库那条后台
        // 线程补上（`import_assets` 会跳过已经探测过的）。
        let assets: HashMap<PathBuf, AssetId> = {
            let Some(project) = self.project.as_mut() else {
                return;
            };
            paths
                .iter()
                .map(|path| (path.clone(), project.add_asset(path)))
                .collect()
        };

        let sequence_id = {
            let Some(project) = self.project.as_mut() else {
                return;
            };
            project.add_sequence(&name, spec)
        };
        self.switch_active_sequence(sequence_id, cx);

        let Some(base) = self
            .project
            .as_ref()
            .map(|project| project.sequence().clone())
        else {
            return;
        };
        let (filled, mut warnings) = build_sequence(base, &imported, &assets);
        self.commit_edit(filled, "从 OTIO 导入");
        warnings.splice(0..0, imported.warnings.iter().cloned());

        let clips = imported
            .tracks
            .iter()
            .map(|track| track.clips.len())
            .sum::<usize>();
        let tracks = imported.tracks.len();
        // 探测与封面走素材库那条现成的后台线程；缺文件也会在那里标成「缺失」。
        self.import_assets(paths, cx);

        let mut message = format!("已从 OTIO 建序列「{name}」：{tracks} 条轨、{clips} 个片段");
        if !warnings.is_empty() {
            message.push_str(&format!("；{}", warnings.join("；")));
        }
        self.flash(message);
        self.preview_at_playhead(cx);
        cx.notify();
    }

    /// 导入序列的规格：OTIO 里写了就用它，没写沿用当前序列。
    fn otio_spec(&self, imported: &ImportedTimeline) -> SequenceSpec {
        let current = self
            .project
            .as_ref()
            .map(|project| project.sequence())
            .map(|sequence| SequenceSpec {
                width: sequence.width,
                height: sequence.height,
                fps: sequence.fps,
            })
            .unwrap_or_default();
        SequenceSpec {
            width: imported.width.unwrap_or(current.width),
            height: imported.height.unwrap_or(current.height),
            fps: imported.fps.unwrap_or(current.fps),
        }
    }
}

/// 读到的时间线里所有素材路径，去重后按首次出现的顺序。
fn otio_paths(imported: &ImportedTimeline) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for track in &imported.tracks {
        for clip in &track.clips {
            if !paths.contains(&clip.path) {
                paths.push(clip.path.clone());
            }
        }
    }
    paths
}

/// 把读到的轨道铺进一条空序列。`base` 是刚建好的那条（自带一条视频 / 音频轨），
/// 它的轨道会被整体替换成 OTIO 里的那几条；所有 id 都在这里重新分配。
///
/// 返回建好的序列与「转场没加上」的原因：对方文件里的转场时长是它自己算的，
/// 落到我们的余量规则上可能不够，这时候宁可退回硬切也要把原因说出来。
fn build_sequence(
    mut base: Sequence,
    imported: &ImportedTimeline,
    assets: &HashMap<PathBuf, AssetId>,
) -> (Sequence, Vec<String>) {
    base.tracks.clear();
    let mut warnings = Vec::new();
    // (轨道 id, 这条轨道上片段按顺序的 id)
    let mut placed: Vec<(Id, Vec<Id>)> = Vec::new();

    for track in &imported.tracks {
        let track_id = base.add_track(track.kind);
        let mut clips = Vec::new();
        let mut ids = Vec::new();
        for clip in &track.clips {
            let Some(asset_id) = assets.get(&clip.path).copied() else {
                continue;
            };
            let id = base.alloc_id();
            let mut item = ClipItem::new(id, asset_id, clip.source_in, clip.source_out, clip.start);
            item.name = clip.name.clone();
            item.speed = clip.speed.clamp(ClipItem::MIN_SPEED, ClipItem::MAX_SPEED);
            item.remap = clip.remap.clone();
            item.hold = clip.hold.max(Tick::ZERO);
            item.effects = clip.effects.clone();
            item.keys = clip.keys.clone();
            item.color = clip.color.clone();
            clips.push(item);
            ids.push(id);
        }
        let subtitle_ids: Vec<Id> = track.subtitles.iter().map(|_| base.alloc_id()).collect();

        let Some(target) = base.track_mut(track_id) else {
            continue;
        };
        if !track.name.is_empty() {
            target.name = track.name.clone();
        }
        target.muted = track.muted;
        target.locked = track.locked;
        target.solo = track.solo;
        if track.kind == TrackKind::Audio && track.gain.is_finite() && track.gain >= 0.0 {
            target.gain = track.gain.min(crate::timeline::MAX_GAIN);
        }
        target.clips = clips;
        target.subtitles = track
            .subtitles
            .iter()
            .zip(subtitle_ids)
            .map(|(item, id)| SubtitleItem {
                id,
                start: item.start,
                end: item.end,
                text: item.text.clone(),
                style: item.style.clone(),
                karaoke: item.karaoke.clone(),
            })
            .collect();
        placed.push((track_id, ids));
    }

    for (track, (_, ids)) in imported.tracks.iter().zip(&placed) {
        for transition in &track.transitions {
            let Some(after_id) = ids.get(transition.after).copied() else {
                continue;
            };
            match crate::timeline::set_transition(
                &base,
                after_id,
                transition.kind,
                transition.duration,
            ) {
                Ok(applied) => {
                    if applied.clamped {
                        warnings.push(format!(
                            "转场时长被余量钳到 {:.2} 秒",
                            applied.duration.secs()
                        ));
                    }
                    base = applied.sequence;
                }
                Err(error) => warnings.push(format!("转场没加上（{error}）")),
            }
        }
    }

    base.repair_next_id();
    (base, warnings)
}
