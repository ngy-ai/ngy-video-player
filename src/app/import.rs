//! 把媒体文件导入时间线：登记素材、探测、插到播放头（撞上了就排到末尾）。

use std::path::Path;

use super::*;
use crate::timeline::{ClipSpec, EditOp, TrackKind, apply as apply_edit};

impl PlayerApp {
    /// 菜单「导入到时间线…」。
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

    fn import_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let mut imported = 0;
        let mut failed = 0;
        for path in &paths {
            if self.insert_media(path) {
                imported += 1;
            } else {
                failed += 1;
            }
        }

        if imported > 0 {
            self.flash(format!("已导入 {imported} 个素材"));
        }
        if failed > 0 {
            self.error = Some(Banner::new(
                "有素材没有导入",
                "这些文件可能不是视频/音频、已经损坏，或者没有可用的轨道。详细原因见日志。",
            ));
        }
        self.poke();
        cx.notify();
    }

    /// 登记并插入一个文件。返回是否至少插入了一段。
    fn insert_media(&mut self, path: &Path) -> bool {
        let Some(project) = self.project.as_mut() else {
            return false;
        };
        let id = match crate::media::register(project, path) {
            Ok(id) => id,
            Err(error) => {
                log_warn!("导入「{}」失败：{error:#}", path.display());
                return false;
            }
        };

        let Some(project) = self.project.as_mut() else {
            return false;
        };
        // 先拿到素材信息再改序列：`asset` 的借用不能跨过 `sequence` 的可变借用。
        let Some(media) = project.asset(id).and_then(|asset| asset.media.clone()) else {
            return false;
        };
        if media.duration <= Tick::ZERO {
            log_warn!("「{}」读不出时长，跳过", path.display());
            return false;
        }

        let has_video = media.width > 0 && !media.video_codec.is_empty();
        let has_audio = !media.audio_codec.is_empty() || media.channels > 0;
        let targets = target_tracks(&project.sequence, has_video, has_audio);
        if targets.is_empty() {
            return false;
        }

        let start = project.ui.playhead;
        let mut inserted = false;
        for track in targets {
            let spec = ClipSpec::new(id, Tick::ZERO, media.duration);
            // 先试播放头；和已有片段撞上就排到序列末尾，总有个地方能放。
            let mut next = None;
            for at in [start, project.sequence.duration()] {
                let op = EditOp::Insert {
                    track,
                    at,
                    clip: spec.clone(),
                };
                if let Ok(sequence) = apply_edit(&project.sequence, &op) {
                    next = Some(sequence);
                    break;
                }
            }
            if let Some(sequence) = next {
                project.sequence = sequence;
                inserted = true;
            }
        }

        self.remember_history();
        inserted
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
