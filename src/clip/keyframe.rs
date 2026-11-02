//! 关键帧探测：把流复制的切点对到随机访问点上。
//!
//! 流复制的切点必须落在关键帧（I 帧，最好是 IDR）上。落在 P/B 帧上时 ffmpeg
//! 照样会切——它只是把目标时间之前的包丢掉——但输出文件的首帧没有参考画面，
//! 播放器会花屏到下一个关键帧。这就是「无损剪切」最容易被忽略的坑：字节确实
//! 一个没改，可开头几秒是废的。
//!
//! 没有 ffprobe 可用（分发的是一个 exe，不想再塞一个几十 MB 的探测工具），只能
//! 让 ffmpeg 自己扫：`-skip_frame nokey` 让解码器跳过非关键帧、只解 I 帧，再用
//! `showinfo` 把时间戳打到 stderr。为了不扫全片，只在目标时间附近开一个窗口。

use anyhow::{Context as _, Result, bail};
use std::io::Read as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 单次窗口扫描的上限。
///
/// 探测在界面线程上跑（点击「开始」那一刻），网络路径或坏文件不能把界面吊死；
/// 超时就当作没探到，导出退回用户设定的时间。
const TIMEOUT: Duration = Duration::from_secs(2);

/// 窗口在目标时间两侧各留多长。GOP 常见 1–10 秒，15 秒能盖住绝大多数片源；
/// 两侧对称是因为「向后对齐」也要用到后一个关键帧，只往前扫就永远找不到它。
const WINDOW: f64 = 15.0;

/// 第一次没找到更早的关键帧时往外扩到多远。老动画、屏录这类长 GOP 片源能到
/// 30 秒以上，不扩的话会错误地退到文件开头。
const WIDE_WINDOW: f64 = 60.0;

/// 一个时间点两侧最近的关键帧。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Keyframes {
    /// 不晚于目标时间的最近关键帧。找不到时退到 0（文件开头复制总是安全的）。
    pub prev: f64,
    /// 晚于目标时间的最近关键帧。片尾之后没有下一个时为 `None`。
    pub next: Option<f64>,
}

impl Keyframes {
    /// 按对齐方向挑实际起点。
    pub fn pick(&self, align: super::KeyframeAlign) -> f64 {
        match align {
            super::KeyframeAlign::Previous => self.prev,
            // 后一个关键帧不存在（目标已经贴片尾）时退回前一个：宁可多含一段，
            // 也不能切出一个空区间。
            super::KeyframeAlign::Next => self.next.unwrap_or(self.prev),
        }
    }
}

/// 探测 `time` 两侧最近的关键帧。
///
/// 只认第一个视频流：裁切也只映射 `0:V:0`，两边的索引必须一致。
pub fn around(executable: &Path, input: &Path, time: f64) -> Result<Keyframes> {
    let time = time.max(0.0);
    let window = |half: f64| scan_window(executable, input, (time - half).max(0.0), time + half);

    let near = window(WINDOW)?;
    // 窗口里有关键帧，或者目标就在片头附近（再往前也没什么可找的）。
    if near.contains_prev(time) || time <= WINDOW {
        return Ok(near.into_keyframes(time));
    }

    // 一个更早的关键帧都没有：要么 GOP 特别长，要么片源的关键帧索引本身不规范。
    // 往外扩一次；还是没有就退到 0。
    let wide = window(WIDE_WINDOW)?;
    Ok(wide.into_keyframes(time))
}

/// 一次窗口扫描的结果。
struct Scan {
    times: Vec<f64>,
}

impl Scan {
    fn contains_prev(&self, time: f64) -> bool {
        self.times.iter().any(|&at| at <= time + 0.001)
    }

    fn into_keyframes(self, time: f64) -> Keyframes {
        // 片源的时间基可能有微小抖动，留 1ms 余量，否则「恰好等于关键帧」的输入
        // 会被判成落在两个关键帧中间。
        let tolerance = 0.001;
        let prev = self
            .times
            .iter()
            .copied()
            .filter(|&at| at <= time + tolerance)
            .fold(None::<f64>, |best, at| {
                Some(best.map_or(at, |best| best.max(at)))
            })
            .unwrap_or(0.0);
        let next = self
            .times
            .iter()
            .copied()
            .filter(|&at| at > time + tolerance)
            .fold(None::<f64>, |best, at| {
                Some(best.map_or(at, |best| best.min(at)))
            });
        Keyframes { prev, next }
    }
}

/// 扫 `from..to` 这一段里的所有关键帧时间戳（绝对时间）。
fn scan_window(executable: &Path, input: &Path, from: f64, to: f64) -> Result<Scan> {
    // `-t` 算的是窗口长度，不是终点：`-ss` / `-t` 都放在 `-i` 前时它们是输入选项。
    let span = (to - from).max(0.1);
    let mut child = Command::new(executable)
        .arg("-hide_banner")
        .arg("-nostdin")
        // showinfo 是 info 级别的日志，`-loglevel error` 会把它一起关掉。
        .arg("-loglevel")
        .arg("info")
        .arg("-ss")
        .arg(format!("{from:.3}"))
        .arg("-t")
        .arg(format!("{span:.3}"))
        // 保留绝对时间戳，否则每一段窗口的输出都从 0 开始，拼不回原片的时间轴。
        .arg("-copyts")
        .arg("-skip_frame")
        .arg("nokey")
        .arg("-i")
        .arg(input)
        .arg("-map")
        .arg("0:V:0?")
        .arg("-vf")
        .arg("showinfo")
        .arg("-f")
        .arg("null")
        .arg("-")
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("探测关键帧失败：无法运行 {}", executable.display()))?;

    // stderr 在后台读走：管道塞满会让 ffmpeg 卡住，而不读日志就没法解析时间戳。
    let stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.read_to_string(&mut text);
        }
        text
    });

    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                bail!(
                    "探测关键帧超时（{} 秒）：{}",
                    TIMEOUT.as_secs(),
                    input.display()
                );
            }
            Err(error) => bail!("等待 ffmpeg 探测进程失败：{error}"),
        }
    }

    // 不解码非关键帧时 ffmpeg 不会失败；这里只关心能不能读到时间戳，
    // 读不到就当这个窗口里没有关键帧，交给调用方退回保守值。
    Ok(Scan {
        times: parse_times(&reader.join().unwrap_or_default()),
    })
}

/// 从 `showinfo` 的日志里抠出 `pts_time:`。
///
/// 一行长这样：
/// `[Parsed_showinfo_0 @ ...] n:0 pts:0 pts_time:0 duration:512 ... type:I`
///
/// 只认带 `pts_time:` 的行：showinfo 的续行里有 `mean:[...]` 之类的方括号，
/// 直接按 token 扫会把数字认错。
fn parse_times(log: &str) -> Vec<f64> {
    let mut times: Vec<f64> = log
        .lines()
        .filter_map(|line| {
            let rest = line.split("pts_time:").nth(1)?;
            let token = rest.split_whitespace().next()?;
            token.parse::<f64>().ok()
        })
        .filter(|time| time.is_finite())
        .collect();
    times.sort_by(f64::total_cmp);
    times.dedup_by(|a, b| (*a - *b).abs() < 0.0005);
    times
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实 showinfo 输出的形状，含续行。
    const LOG: &str = "\
[Parsed_showinfo_0 @ 000001] n:   0 pts:      0 pts_time:0       duration:    512 duration_time:0.04    pos:      48 fmt:yuv420p sar:1/1 s:1920x1080 i:P iskey:1 type:I checksum:67B8
   plane_checksum:[EB2F 3B3F 8500] mean:[126 127 126] stdev:[55.9 79.5]
[Parsed_showinfo_0 @ 000001] n:   1 pts: 128000 pts_time:10      duration:    512 duration_time:0.04    pos:18474142 fmt:yuv420p sar:1/1 s:1920x1080 i:P iskey:1 type:I checksum:5FB8
[Parsed_showinfo_0 @ 000001] n:   2 pts: 256000 pts_time:12.345  duration:    512 duration_time:0.04    pos:37371136 fmt:yuv420p sar:1/1 s:1920x1080 i:P iskey:1 type:I checksum:CEED
";

    #[test]
    fn 从_showinfo_日志里抠出时间戳() {
        assert_eq!(parse_times(LOG), vec![0.0, 10.0, 12.345]);
    }

    #[test]
    fn 没有关键帧时是空列表() {
        assert!(parse_times("").is_empty());
        assert!(parse_times("Output #0, null, to 'pipe:'").is_empty());
    }

    #[test]
    fn 重复与倒序的时间戳会被整理() {
        let log = "x pts_time:12.345 y\nx pts_time:0 y\nx pts_time:12.3450 y\n";
        assert_eq!(parse_times(log), vec![0.0, 12.345]);
    }

    #[test]
    fn 两侧关键帧按最近的挑() {
        let scan = Scan {
            times: vec![0.0, 10.0, 20.0, 30.0],
        };
        let frames = scan.into_keyframes(12.5);
        assert_eq!(frames.prev, 10.0);
        assert_eq!(frames.next, Some(20.0));
    }

    #[test]
    fn 正好落在关键帧上时算作前一个() {
        let scan = Scan {
            times: vec![0.0, 10.0, 20.0],
        };
        let frames = scan.into_keyframes(10.0);
        assert_eq!(frames.prev, 10.0);
        assert_eq!(frames.next, Some(20.0));
    }

    #[test]
    fn 一个关键帧都没有时退回文件开头() {
        let frames = Scan { times: Vec::new() }.into_keyframes(42.0);
        assert_eq!(frames.prev, 0.0);
        assert_eq!(frames.next, None);
    }

    /// 真跑一次 ffmpeg，验证拼出来的命令确实能扫到关键帧。
    ///
    /// 需要本机 ffmpeg 和一个片源，默认跳过：
    /// `NGY_KEYFRAME_FIXTURE=path/to/video.mkv cargo test --lib 真实探测 -- --ignored`
    #[test]
    #[ignore]
    fn 真实探测能分出前后关键帧() {
        let Ok(input) = std::env::var("NGY_KEYFRAME_FIXTURE") else {
            return;
        };
        let executable = crate::platform::ffmpeg::executable().expect("需要 ffmpeg");
        let frames = around(&executable, Path::new(&input), 25.0).expect("探测应成功");
        assert!(frames.prev <= 25.0, "前一个关键帧必须在目标之前");
        if let Some(next) = frames.next {
            assert!(next > 25.0, "后一个关键帧必须在目标之后");
        }
    }

    #[test]
    fn 片尾之后没有下一个关键帧() {
        let scan = Scan {
            times: vec![0.0, 10.0],
        };
        let frames = scan.into_keyframes(50.0);
        assert_eq!(frames.prev, 10.0);
        assert_eq!(frames.next, None);
        // 后一个不存在时退回前一个，不切出空区间。
        assert_eq!(frames.pick(super::super::KeyframeAlign::Next), 10.0);
    }
}
