//! 代理文件：轻量副本，给剪辑软件做「代理剪辑」用。
//!
//! 4K 原片在时间线上拖动会卡，先转一份小文件剪，最后套回原片（预览的选用规则在
//! `timeline::proxy`）。代理只求解码快、不求画质，所以固定 H.264 + veryfast，
//! 按 [`ProxyQuality`] 的档位等比缩放并配对应的 CRF。
//!
//! 输出 `<原名>.proxy.mp4`，与原片同目录；不改动原文件，也不写字幕与章节。

use anyhow::{Context as _, Result};
use futures::channel::mpsc::UnboundedSender;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 代理文件的后缀。
const SUFFIX: &str = "proxy";

/// 代理的规格档：只决定预览清晰度与代理文件的体积，导出始终用原片。
///
/// 默认 480p：代理的用处是「解码快、文件小」，往上一档就多几倍像素要解；
/// 只有 4K 进 4K 序列、或者要看清细部（对位、抠像）时才值得加档。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProxyQuality {
    #[default]
    P480,
    P720,
    P1080,
}

impl ProxyQuality {
    pub const ALL: [Self; 3] = [Self::P480, Self::P720, Self::P1080];

    pub fn label(self) -> &'static str {
        match self {
            Self::P480 => "480p",
            Self::P720 => "720p",
            Self::P1080 => "1080p",
        }
    }

    /// 缩放后的高度。宽度按保持偶数算（libx264 的 yuv420p 要求偶数）。
    pub fn height(self) -> u32 {
        match self {
            Self::P480 => 480,
            Self::P720 => 720,
            Self::P1080 => 1080,
        }
    }

    /// 档越高 CRF 越小：480p 本来就糊，给 28 足够；1080p 要留点余量给细部。
    fn crf(self) -> u32 {
        match self {
            Self::P480 => 28,
            Self::P720 => 26,
            Self::P1080 => 24,
        }
    }

    /// 码率封顶（kbps）。CRF 管质量、这一项管「别大到拖动时解不动」：花样多、
    /// 噪声重的画面在纯 CRF 下能飙到几十兆，代理就白做了。
    pub fn max_bitrate(self) -> u32 {
        match self {
            Self::P480 => 1_500,
            Self::P720 => 3_000,
            Self::P1080 => 6_000,
        }
    }

    /// 等比缩到这一档。`-2` 让宽度按保持偶数取整。
    pub fn scale_filter(self) -> String {
        format!("scale=-2:{}:flags=fast_bilinear", self.height())
    }
}

/// 一次代理生成请求。
#[derive(Clone, Debug)]
pub struct ProxyRequest {
    pub input: PathBuf,
    pub output: PathBuf,
    /// 原片时长（秒），进度条的分母。
    pub duration: f64,
    pub quality: ProxyQuality,
}

#[derive(Clone, Debug)]
pub enum ProxyEvent {
    Progress(f32),
    Done(ProxyOutcome),
}

#[derive(Clone, Debug)]
pub enum ProxyOutcome {
    Finished(PathBuf),
    Cancelled,
    Failed(String),
}

pub struct ProxyJob {
    child: Arc<Mutex<Option<Child>>>,
    cancelled: Arc<AtomicBool>,
}

impl ProxyJob {
    pub fn start(request: ProxyRequest, tx: UnboundedSender<ProxyEvent>) -> Result<Self> {
        let executable = crate::platform::ffmpeg::executable()?;
        let mut child = build_command(&executable, &request)
            .spawn()
            .with_context(|| format!("启动 ffmpeg 失败：{}", executable.display()))?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let child = Arc::new(Mutex::new(Some(child)));
        let cancelled = Arc::new(AtomicBool::new(false));

        let tail = Arc::new(Mutex::new(VecDeque::new()));
        if let Some(stderr) = stderr {
            let tail = tail.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(|line| line.ok()) {
                    let mut tail = tail.lock().unwrap();
                    if tail.len() >= 12 {
                        tail.pop_front();
                    }
                    tail.push_back(line);
                }
            });
        }

        let task_child = child.clone();
        let task_cancelled = cancelled.clone();
        let output = request.output.clone();
        let total = request.duration;
        std::thread::spawn(move || {
            if let Some(stdout) = stdout {
                read_progress(stdout, total, &tx);
            }
            let status = crate::platform::ffmpeg::wait_polling(&task_child);
            let cancelled = task_cancelled.load(Ordering::SeqCst);
            let outcome = if cancelled {
                let _ = std::fs::remove_file(&output);
                ProxyOutcome::Cancelled
            } else {
                match status {
                    Some(Ok(status)) if status.success() => ProxyOutcome::Finished(output),
                    Some(Ok(status)) => {
                        let detail = tail
                            .lock()
                            .unwrap()
                            .iter()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("\n");
                        ProxyOutcome::Failed(if detail.is_empty() {
                            format!("ffmpeg 异常退出（{:?}）", status.code())
                        } else {
                            format!("ffmpeg 异常退出（{:?}）\n{detail}", status.code())
                        })
                    }
                    Some(Err(error)) => {
                        ProxyOutcome::Failed(format!("等待 ffmpeg 结束失败：{error}"))
                    }
                    None => ProxyOutcome::Failed("ffmpeg 进程已经结束".to_string()),
                }
            };
            let _ = tx.unbounded_send(ProxyEvent::Done(outcome));
        });

        Ok(Self { child, cancelled })
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
    }
}

impl Drop for ProxyJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// 代理落点：与原片同目录，`<原名>.proxy.mp4`，撞名就编号。
pub fn output_path(input: &Path) -> PathBuf {
    let dir = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| "代理".to_string());
    let base = format!("{stem}.{SUFFIX}");

    let mut candidate = dir.join(format!("{base}.mp4"));
    let mut index = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{base} ({index}).mp4"));
        index += 1;
        if index > 999 {
            break;
        }
    }
    candidate
}

fn build_command(executable: &Path, request: &ProxyRequest) -> Command {
    let mut command = Command::new(executable);
    command
        .arg("-hide_banner")
        .arg("-nostdin")
        .arg("-y")
        .arg("-nostats")
        .arg("-loglevel")
        .arg("error")
        .arg("-progress")
        .arg("pipe:1")
        .arg("-i")
        .arg(&request.input)
        .arg("-map")
        .arg("0:V:0?")
        .arg("-map")
        .arg("0:a?")
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("veryfast")
        .arg("-crf")
        .arg(request.quality.crf().to_string())
        .arg("-maxrate")
        .arg(format!("{}k", request.quality.max_bitrate()))
        .arg("-bufsize")
        .arg(format!("{}k", request.quality.max_bitrate() * 2))
        .arg("-vf")
        .arg(request.quality.scale_filter())
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-c:a")
        .arg("aac")
        .arg("-b:a")
        .arg("128k")
        // 剪辑软件拖动时间线会频繁 seek，索引放文件头能少等一次全文件扫描。
        .arg("-movflags")
        .arg("+faststart")
        .arg(&request.output)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn read_progress(stdout: impl Read, total: f64, tx: &UnboundedSender<ProxyEvent>) {
    let total_us = total * 1_000_000.0;
    if total_us <= 0.0 {
        return;
    }
    for line in BufReader::new(stdout).lines().map_while(|line| line.ok()) {
        let Some(value) = line.strip_prefix("out_time_us=") else {
            continue;
        };
        let Ok(micros) = value.trim().parse::<f64>() else {
            continue;
        };
        let progress = (micros / total_us).clamp(0.0, 1.0) as f32;
        let _ = tx.unbounded_send(ProxyEvent::Progress(progress));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ProxyRequest {
        ProxyRequest {
            input: PathBuf::from("课程 01.mkv"),
            output: PathBuf::from("课程 01.proxy.mp4"),
            duration: 600.0,
            quality: ProxyQuality::P480,
        }
    }

    fn args_for(quality: ProxyQuality) -> Vec<String> {
        let mut request = request();
        request.quality = quality;
        build_command(Path::new("ffmpeg"), &request)
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn args() -> Vec<String> {
        build_command(Path::new("ffmpeg"), &request())
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn has(args: &[String], needle: &[&str]) -> bool {
        args.windows(needle.len()).any(|slice| {
            slice
                .iter()
                .zip(needle)
                .all(|(arg, want)| arg.as_str() == *want)
        })
    }

    #[test]
    fn 代理固定为_480p_快速编码() {
        let args = args();
        assert!(has(&args, &["-vf", "scale=-2:480:flags=fast_bilinear"]));
        assert!(has(&args, &["-preset", "veryfast"]));
        assert!(has(&args, &["-crf", "28"]));
        assert!(has(&args, &["-c:a", "aac"]));
        assert!(has(&args, &["-movflags", "+faststart"]));
        // 音频轨可缺省，视频轨必须映射。
        assert!(has(&args, &["-map", "0:V:0?"]));
        assert!(has(&args, &["-map", "0:a?"]));
    }

    #[test]
    fn 档位决定缩放高度与_crf() {
        assert!(has(
            &args_for(ProxyQuality::P720),
            &["-vf", "scale=-2:720:flags=fast_bilinear"]
        ));
        assert!(has(&args_for(ProxyQuality::P720), &["-crf", "26"]));
        assert!(has(
            &args_for(ProxyQuality::P1080),
            &["-vf", "scale=-2:1080:flags=fast_bilinear"]
        ));
        assert!(has(&args_for(ProxyQuality::P1080), &["-crf", "24"]));
        // 档位越高越清晰：高度递增（CRF 随之递减，见上面两条断言）。
        let heights: Vec<u32> = ProxyQuality::ALL
            .iter()
            .map(|quality| quality.height())
            .collect();
        assert_eq!(heights, vec![480, 720, 1080]);
    }

    #[test]
    fn 码率按档封顶() {
        // CRF 之外再封顶：噪声重的画面在纯 CRF 下能飙到几十兆，代理就白做了。
        for quality in ProxyQuality::ALL {
            let bitrate = quality.max_bitrate();
            let args = args_for(quality);
            assert!(has(&args, &["-maxrate", &format!("{bitrate}k")]));
            assert!(has(&args, &["-bufsize", &format!("{}k", bitrate * 2)]));
        }
        // 档越高容许的码率越高。
        assert!(ProxyQuality::P480.max_bitrate() < ProxyQuality::P720.max_bitrate());
        assert!(ProxyQuality::P720.max_bitrate() < ProxyQuality::P1080.max_bitrate());
    }

    #[test]
    fn 代理输出不覆盖已有文件() {
        let dir = std::env::temp_dir().join(format!("ngy-proxy-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("课程 01.mkv");
        std::fs::write(&input, b"x").unwrap();

        let first = output_path(&input);
        assert_eq!(first, dir.join("课程 01.proxy.mp4"));
        std::fs::write(&first, b"x").unwrap();

        let second = output_path(&input);
        assert_eq!(second, dir.join("课程 01.proxy (2).mp4"));

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
