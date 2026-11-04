//! 音频波形：ffmpeg 解码成单声道 8kHz 的 f32 流，边读边按块取峰值。
//!
//! 「不做整条素材一次性解码」的意思是：不把几十分钟的采样攒成一个 `Vec<f32>`
//! 再处理，而是每读到 64KB 就折算成峰值块。一小时的素材在内存里只留 36 万个
//! 峰值（约 1.4MB），和序列长度无关。
//!
//! 峰值落盘成 `.bin`（魔数 + 采样率 + 长度 + f32），内存里再套一层 LRU：
//! 拖动时间线时同一素材反复取波形，不必每次都读盘。

use super::key::MediaKey;
use super::lru::Lru;
use crate::platform::ffmpeg;
use anyhow::{Context as _, Result, anyhow, bail};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

/// 每秒多少个峰值块。100 在 1080p 时间线上一像素一个块左右，够细也够省。
pub const PEAKS_PER_SECOND: u32 = 100;
/// 解码后的采样率。波形只看包络，8kHz 足够，还能省一大截解码时间。
pub const SAMPLE_RATE: u32 = 8_000;

/// 默认常驻多少条素材的波形。
pub const DEFAULT_CAPACITY: usize = 64;

/// 生成参数变了就 +1。
const CACHE_VERSION: u32 = 1;
const MAGIC: &[u8; 8] = b"NGYWAVE1";
const HEADER_LEN: usize = 16;

/// 波形缓存：内存 LRU + 磁盘 `.bin`，容量可配。
pub struct WaveformCache {
    root: PathBuf,
    entries: Mutex<Lru<MediaKey, Arc<Vec<f32>>>>,
}

impl WaveformCache {
    /// `capacity` 是能同时留在内存里的素材条数；0 表示只用磁盘、不缓存。
    pub fn new(root: PathBuf, capacity: usize) -> Self {
        Self {
            root,
            entries: Mutex::new(Lru::new(capacity)),
        }
    }

    /// 默认缓存根目录 + 默认容量。
    pub fn with_defaults() -> Self {
        Self::new(super::cache_root(), DEFAULT_CAPACITY)
    }

    pub fn capacity(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .capacity()
    }

    /// 当前常驻内存的素材条数。界面状态栏与测试都用得上。
    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 取一条素材的波形。内存没有就读盘，磁盘没有就解码生成。
    pub fn load(&self, input: &Path) -> Result<Arc<Vec<f32>>> {
        let key =
            MediaKey::of(input).ok_or_else(|| anyhow!("读不到素材信息：{}", input.display()))?;
        {
            let mut entries = self
                .entries
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(peaks) = entries.get(&key).cloned() {
                return Ok(peaks);
            }
        }

        let path = peaks_path(&self.root, key);
        let peaks = match read_peaks(&path) {
            Some(peaks) => Arc::new(peaks),
            None => {
                let peaks = Arc::new(generate(input)?);
                if let Err(error) = write_peaks(&path, &peaks) {
                    // 缓存写不进去（磁盘满、目录只读）不该让波形整个不可用。
                    log_debug!("写波形缓存失败：{error:#}");
                }
                peaks
            }
        };

        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        entries.insert(key, Arc::clone(&peaks));
        Ok(peaks)
    }
}

/// 把采样按 `block` 个一块取最大绝对值。最后不足一块的也收进去。
pub fn aggregate(samples: impl IntoIterator<Item = f32>, block: usize) -> Vec<f32> {
    let mut peaks = Peaks::new(block);
    for sample in samples {
        peaks.push(sample);
    }
    peaks.finish()
}

/// 流式峰值聚合器。每块只留一个最大值，内存与素材长度无关。
struct Peaks {
    block: usize,
    current: f32,
    count: usize,
    values: Vec<f32>,
}

impl Peaks {
    fn new(block: usize) -> Self {
        Self {
            block: block.max(1),
            current: 0.0,
            count: 0,
            values: Vec::new(),
        }
    }

    fn push(&mut self, sample: f32) {
        let magnitude = sample.abs();
        if magnitude.is_finite() && magnitude > self.current {
            self.current = magnitude;
        }
        self.count += 1;
        if self.count >= self.block {
            self.values.push(self.current);
            self.current = 0.0;
            self.count = 0;
        }
    }

    fn finish(mut self) -> Vec<f32> {
        if self.count > 0 {
            self.values.push(self.current);
        }
        self.values
    }
}

fn peaks_path(root: &Path, key: MediaKey) -> PathBuf {
    root.join(key.dir_name())
        .join(format!("peaks-v{CACHE_VERSION}-{PEAKS_PER_SECOND}.bin"))
}

fn generate(input: &Path) -> Result<Vec<f32>> {
    let executable = ffmpeg::executable()?;
    let mut child = Command::new(&executable)
        .args(["-hide_banner", "-nostdin", "-v", "error", "-i"])
        .arg(input)
        .args(["-vn", "-ac", "1", "-ar"])
        .arg(SAMPLE_RATE.to_string())
        .args(["-f", "f32le", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("启动 ffmpeg 解码音频失败：{}", executable.display()))?;

    // stderr 要有人收：`-v error` 虽然话少，但管道塞满时子进程会卡住。
    let stderr_reader = child.stderr.take().map(|mut stderr| {
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        })
    });

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("ffmpeg 没有交出音频管道"))?;
    let block = (SAMPLE_RATE / PEAKS_PER_SECOND) as usize;
    let mut peaks = Peaks::new(block);
    let mut buffer = vec![0u8; 64 * 1024];
    // 一次读到的字节数不保证是 4 的倍数，跨块的半个 f32 留到下一轮。
    let mut leftover: Vec<u8> = Vec::new();

    loop {
        let read = stdout
            .read(&mut buffer)
            .with_context(|| format!("读取 ffmpeg 音频输出失败：{}", input.display()))?;
        if read == 0 {
            break;
        }
        leftover.extend_from_slice(&buffer[..read]);
        let complete = leftover.len() / 4 * 4;
        for chunk in leftover[..complete].chunks_exact(4) {
            peaks.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        leftover.drain(..complete);
    }

    let status = child.wait().context("等待 ffmpeg 解码音频")?;
    let message = stderr_reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    if !status.success() {
        bail!("解码音频失败：{}：{}", input.display(), message.trim());
    }
    Ok(peaks.finish())
}

fn read_peaks(path: &Path) -> Option<Vec<f32>> {
    let bytes = fs::read(path).ok()?;
    decode_peaks(&bytes, PEAKS_PER_SECOND)
}

fn write_peaks(path: &Path, peaks: &[f32]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建波形缓存目录失败：{}", parent.display()))?;
    }
    let bytes = encode_peaks(peaks, PEAKS_PER_SECOND);
    let mut temp_name = path.as_os_str().to_os_string();
    temp_name.push(".tmp");
    let temp = PathBuf::from(temp_name);
    fs::write(&temp, &bytes)?;
    // Windows 上 rename 不能覆盖已有文件；缓存坏了就换新的。
    let _ = fs::remove_file(path);
    fs::rename(&temp, path).with_context(|| format!("写入波形缓存失败：{}", path.display()))?;
    Ok(())
}

fn encode_peaks(peaks: &[f32], rate: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER_LEN + peaks.len() * 4);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(peaks.len() as u32).to_le_bytes());
    for peak in peaks {
        bytes.extend_from_slice(&peak.to_le_bytes());
    }
    bytes
}

fn decode_peaks(bytes: &[u8], expected_rate: u32) -> Option<Vec<f32>> {
    if bytes.len() < HEADER_LEN || &bytes[..8] != MAGIC {
        return None;
    }
    let rate = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    if rate != expected_rate {
        return None;
    }
    let count = u32::from_le_bytes(bytes[12..16].try_into().ok()?) as usize;
    let expected = count.checked_mul(4)?;
    let body = &bytes[HEADER_LEN..];
    if body.len() < expected {
        return None;
    }
    Some(
        body[..expected]
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ngy-vp-wave-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("创建临时目录");
        dir
    }

    #[test]
    fn 每块取最大绝对值() {
        let peaks = aggregate([0.1, -0.5, 0.2, -0.3], 2);
        assert_eq!(peaks, [0.5, 0.3]);
    }

    #[test]
    fn 最后不足一块也收进去() {
        let peaks = aggregate([0.1, 0.2, 0.3, 0.4], 3);
        assert_eq!(peaks, [0.3, 0.4]);
    }

    #[test]
    fn 空输入与零块大小都不会炸() {
        assert!(aggregate([], 100).is_empty());
        assert_eq!(aggregate([0.7], 0), [0.7], "块大小至少按 1 算");
    }

    #[test]
    fn 波形文件往返且拒绝坏数据() {
        let peaks = vec![0.0, 0.25, 1.0];
        let bytes = encode_peaks(&peaks, PEAKS_PER_SECOND);
        assert_eq!(decode_peaks(&bytes, PEAKS_PER_SECOND), Some(peaks));

        assert_eq!(
            decode_peaks(&bytes, PEAKS_PER_SECOND + 1),
            None,
            "采样率对不上就不是同一份缓存"
        );
        assert_eq!(decode_peaks(&bytes[..12], PEAKS_PER_SECOND), None);
        assert_eq!(decode_peaks(b"NOTAWAVE", PEAKS_PER_SECOND), None);

        let mut truncated = bytes.clone();
        truncated.pop();
        assert_eq!(
            decode_peaks(&truncated, PEAKS_PER_SECOND),
            None,
            "半截数据不能用"
        );
    }

    #[test]
    fn 磁盘命中时不启动_ffmpeg() {
        let dir = scratch("disk");
        let input = dir.join("a.mkv");
        fs::write(&input, b"fake audio").expect("写文件");

        let cache = dir.join("cache");
        let key = MediaKey::of(&input).expect("读指纹");
        let path = peaks_path(&cache, key);
        write_peaks(&path, &[0.2, 0.8]).expect("写假波形");

        let store = WaveformCache::new(cache, 2);
        let peaks = store.load(&input).expect("磁盘命中不该需要 ffmpeg");
        assert_eq!(&*peaks, &[0.2, 0.8]);
        assert_eq!(store.len(), 1, "读一次之后常驻内存");
        assert!(!store.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 容量为零时只读磁盘不常驻() {
        let dir = scratch("capacity");
        let input = dir.join("a.mkv");
        fs::write(&input, b"fake audio").expect("写文件");

        let cache = dir.join("cache");
        let key = MediaKey::of(&input).expect("读指纹");
        let path = peaks_path(&cache, key);
        write_peaks(&path, &[0.5]).expect("写假波形");

        let store = WaveformCache::new(cache, 0);
        assert_eq!(store.capacity(), 0);
        assert_eq!(&*store.load(&input).expect("读波形"), &[0.5]);
        assert_eq!(&*store.load(&input).expect("读波形"), &[0.5]);

        let _ = fs::remove_dir_all(&dir);
    }

    // 需要本机 ffmpeg：`cargo test -- --ignored`。
    #[test]
    #[ignore = "需要本机 ffmpeg：cargo test -- --ignored"]
    fn 真实音频生成波形并落盘() {
        let dir = scratch("real");
        let input = dir.join("tone.mp4");
        let executable = ffmpeg::executable().expect("本机没有 ffmpeg");
        let status = Command::new(&executable)
            .args([
                "-hide_banner",
                "-nostdin",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=2",
                "-c:a",
                "aac",
            ])
            .arg(&input)
            .status()
            .expect("生成测试音频");
        assert!(status.success(), "ffmpeg 生成音频失败");

        let store = WaveformCache::new(dir.join("cache"), 4);
        let peaks = store.load(&input).expect("生成波形");
        assert!(
            (150..=250).contains(&peaks.len()),
            "两秒素材在一百峰/秒下应当约 200 个块：{}",
            peaks.len()
        );
        assert!(
            peaks.iter().copied().fold(0.0f32, f32::max) > 0.1,
            "正弦波不该是静音"
        );

        let path = peaks_path(&dir.join("cache"), MediaKey::of(&input).unwrap());
        assert!(path.is_file(), "波形应当落盘");

        let _ = fs::remove_dir_all(&dir);
    }
}
