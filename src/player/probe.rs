//! 「查看格式」用的文件探测。
//!
//! 刻意不借正在播放的那个引擎：探测要 `loadfile`，会把用户正在看的片子顶掉。
//! 这里另开一个一次性实例，只解复用、不解码、不出声，把容器和轨道读出来就销毁。
//! 代价是每次探测多一份 mpv 实例的生命周期（本地文件几十毫秒），换来的是
//! 「看格式」这个只读操作绝不碰播放状态。

use super::ffi::{self, MpvApi, MpvHandle, event};
use anyhow::{Result, bail};
use std::{
    ffi::CString,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

/// 探测失败前最多等多久。网络路径/坏文件不该把界面线程之外的探测线程吊死。
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// 探测最常撞上的几个 mpv 错误码（`mpv/client.h`）。这些码的原文是英文，
/// 界面上直接甩给用户没有指导意义，所以挑出这几个换成能看懂的说明。
const NOTHING_TO_PLAY: i32 = -16;
const UNKNOWN_FORMAT: i32 = -17;
const LOADING_FAILED: i32 = -13;

/// 一条轨道在界面上要展示的全部信息。
#[derive(Clone, Debug)]
pub struct MediaTrack {
    /// 「视频」/「音频」/「字幕」，直接拿去显示。
    pub kind: &'static str,
    pub codec: String,
    pub language: String,
    pub title: String,
    /// 分辨率、声道、采样率、码率之类，已经按轨道类型拼好。
    pub detail: String,
    /// 外挂字幕/音轨（相对容器内封的那几条）。
    pub external: bool,
}

#[derive(Clone, Debug)]
pub struct MediaInfo {
    /// 容器名，例如 Matroska、MPEG-TS。
    pub container: String,
    pub duration: f64,
    pub size: Option<u64>,
    pub tracks: Vec<MediaTrack>,
}

/// 探测一个文件。错误信息已经是给人看的中文句子，界面直接展示即可。
pub fn probe(path: &Path) -> Result<MediaInfo, String> {
    probe_inner(path).map_err(|error| format!("{error:#}"))
}

fn probe_inner(path: &Path) -> Result<MediaInfo> {
    if !path.exists() {
        bail!("文件已不存在：{}", path.display());
    }

    let api = MpvApi::load()?;
    let handle = unsafe { (api.create)() };
    if handle.is_null() {
        bail!("mpv_create 返回空指针");
    }
    // 任何分支（含 `?` 提前返回）都要把实例销毁，否则每探一次漏一个后台线程。
    let _guard = HandleGuard {
        api: Arc::clone(&api),
        handle,
    };

    // 只解复用：不解码、不出声、不建窗口。这样打开 4K 大文件也只读索引。
    //
    // `vid`/`aid`/`sid` 不能设成 `no` 来省事：mpv 把「一条轨道都没选中」当成没有
    // 可播内容，直接结束会话（END_FILE + `no audio or video data played`），
    // 连 FILE_LOADED 都不发、track-list 也一并清空，探测反而什么都读不到。
    // 保留正常选轨、把输出接到 null，再用 `pause` 停在第一帧之前，效果一样。
    for (name, value) in [
        ("config", "no"),
        ("terminal", "no"),
        ("vo", "null"),
        ("ao", "null"),
        ("pause", "yes"),
        ("audio-display", "no"),
        ("idle", "yes"),
        ("force-window", "no"),
        ("load-scripts", "no"),
        ("osc", "no"),
        ("input-default-bindings", "no"),
    ] {
        set_option(&api, handle, name, value);
    }

    let rc = unsafe { (api.initialize)(handle) };
    if rc < 0 {
        bail!("初始化探测实例失败：{}", api.error_text(rc));
    }

    let url = path.to_string_lossy().to_string();
    api.run_command(handle, &["loadfile", &url, "replace"])?;
    wait_loaded(&api, handle, path)?;

    let container = api
        .property_string(handle, "file-format")
        .unwrap_or_default();
    let duration = api.property_f64(handle, "duration").unwrap_or(0.0);
    let size = std::fs::metadata(path).ok().map(|meta| meta.len());

    Ok(MediaInfo {
        container,
        duration,
        size,
        tracks: read_tracks(&api, handle),
    })
}

/// 等到 `FILE_LOADED`；中途 `END_FILE` 说明解复用都失败，直接把 mpv 的理由带回去。
fn wait_loaded(api: &MpvApi, handle: *mut MpvHandle, path: &Path) -> Result<()> {
    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        let event = unsafe { (api.wait_event)(handle, 0.25) };
        if event.is_null() {
            continue;
        }
        match unsafe { (*event).event_id } {
            event::FILE_LOADED => return Ok(()),
            event::END_FILE => {
                let data = unsafe { (*event).data as *const ffi::MpvEventEndFile };
                if data.is_null() {
                    bail!("无法解析 {}：未知原因", path.display());
                }
                let reason = describe_error(api, unsafe { (*data).error });
                bail!("无法解析 {}：{reason}", path.display());
            }
            _ => {}
        }
        if Instant::now() > deadline {
            bail!("读取文件信息超时：{}", path.display());
        }
    }
}

/// 把探测失败的错误码翻成中文，并尽量指出下一步该查什么。
fn describe_error(api: &MpvApi, code: i32) -> String {
    match code {
        NOTHING_TO_PLAY => "文件里没有可播放的音频或视频轨道".to_string(),
        UNKNOWN_FORMAT => "无法识别的文件格式（可能不是媒体文件，或者文件不完整）".to_string(),
        LOADING_FAILED => "打开文件失败（可能被占用、权限不足或已损坏）".to_string(),
        _ => api.error_text(code),
    }
}

/// `track-list` 的属性逐条按名字读。mpv 的属性接口比解析 node 树省事得多，
/// 少一层布局假设也就少一处随版本变化的风险。
fn read_tracks(api: &MpvApi, handle: *mut MpvHandle) -> Vec<MediaTrack> {
    let count = api
        .property_i64(handle, "track-list/count")
        .unwrap_or(0)
        .clamp(0, 64);
    let mut tracks = Vec::new();

    for index in 0..count {
        let key = |name: &str| format!("track-list/{index}/{name}");
        // 封面图（mp4 的 attached_pic，例如「专辑封面」）在 mpv 里也占一条 video 轨，
        // 但它不是内容轨，混进列表只会让用户以为片子有两路画面。
        if api.property_flag(handle, &key("image")).unwrap_or(false) {
            continue;
        }
        let kind = match api.property_string(handle, &key("type")).as_deref() {
            Some("video") => "视频",
            Some("audio") => "音频",
            Some("sub") => "字幕",
            // 附件、封面之类不占一行的轨道直接跳过。
            _ => continue,
        };

        let mut parts: Vec<String> = Vec::new();
        if kind == "视频" {
            let width = api.property_i64(handle, &key("demux-w"));
            let height = api.property_i64(handle, &key("demux-h"));
            if let (Some(width), Some(height)) = (width, height)
                && width > 0
                && height > 0
            {
                parts.push(format!("{width}×{height}"));
            }
            if let Some(fps) = api.property_f64(handle, &key("demux-fps"))
                && fps > 0.0
            {
                parts.push(format!("{fps:.2} fps"));
            }
        } else if kind == "音频" {
            if let Some(channels) = api.property_i64(handle, &key("demux-channel-count"))
                && channels > 0
            {
                parts.push(format!("{channels} 声道"));
            }
            if let Some(rate) = api.property_i64(handle, &key("demux-samplerate"))
                && rate > 0
            {
                parts.push(format!("{:.1} kHz", rate as f64 / 1000.0));
            }
        }
        // 不同 mpv 版本给的属性名不完全一致，两个都试一下。
        let bitrate = api
            .property_i64(handle, &key("bitrate"))
            .or_else(|| api.property_i64(handle, &key("demux-bitrate")));
        if let Some(bitrate) = bitrate
            && bitrate > 0
        {
            parts.push(format_bitrate(bitrate as f64));
        }

        tracks.push(MediaTrack {
            kind,
            codec: api
                .property_string(handle, &key("codec"))
                .unwrap_or_default(),
            language: api
                .property_string(handle, &key("lang"))
                .unwrap_or_default(),
            title: api
                .property_string(handle, &key("title"))
                .unwrap_or_default(),
            detail: parts.join(" · "),
            external: api.property_flag(handle, &key("external")).unwrap_or(false),
        });
    }

    tracks
}

fn format_bitrate(bits_per_second: f64) -> String {
    if bits_per_second >= 1_000_000.0 {
        format!("{:.1} Mb/s", bits_per_second / 1_000_000.0)
    } else {
        format!("{:.0} kb/s", bits_per_second / 1000.0)
    }
}

fn set_option(api: &MpvApi, handle: *mut MpvHandle, name: &str, value: &str) {
    let (Ok(name), Ok(value)) = (CString::new(name), CString::new(value)) else {
        return;
    };
    let rc = unsafe { (api.set_option_string)(handle, name.as_ptr(), value.as_ptr()) };
    if rc < 0 {
        log_warn!(
            "探测实例不接受选项 {name:?}={value:?}: {}",
            api.error_text(rc)
        );
    }
}

struct HandleGuard {
    api: Arc<MpvApi>,
    handle: *mut MpvHandle,
}

impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe { (self.api.terminate_destroy)(self.handle) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 不存在检查排在加载 libmpv 之前，所以这条不需要库也能跑。
    #[test]
    fn 文件不存在时直接报错() {
        let error = probe(Path::new("probe-missing-file.mp4")).unwrap_err();
        assert!(error.contains("文件已不存在"), "{error}");
    }
}
