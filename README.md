# 影匣 · ngy-video-player

跨平台视频播放器。界面用 [gpui-kit](https://crates.io/crates/gpui-kit)（Zed 的 GPUI），
解码交给 libmpv。Windows / macOS / Ubuntu 同一套代码。

## 为什么是 libmpv

「支持所有已知的视频格式」这件事没有捷径：格式的复杂度不在播放器，而在解复用器、
解码器、色彩转换和字幕排版里。自研一条能覆盖 MKV/H.265/AV1/蓝光/网络流的路，
等于重写一遍 FFmpeg。

所以分工是这样的：

| 层 | 负责 | 为什么 |
| --- | --- | --- |
| gpui-kit | 窗口、布局、输入、文字渲染 | 跨平台一致的 UI 与输入处理 |
| libmpv | 解复用、解码、音视频同步、音频输出、字幕（libass） | mpv 的格式覆盖面就是 FFmpeg 的覆盖面，再加蓝光、网络协议、字幕样式 |

libmpv 内置了完整的 FFmpeg 与 libass，因此「能放什么」直接等于 mpv 能放什么：
MKV / MP4 / MOV / AVI / WMV / FLV / TS / M2TS / MPEG / VOB / OGV / 3GP / RMVB /
WebM / MXF / 各种 ISO 蓝光原盘，以及 H.264 / H.265 / AV1 / VP9 / ProRes / DNxHD /
MPEG-2 等编码；纯音频（FLAC / APE / DSD / Opus / AAC / MP3 …）也走同一条路。

## 取帧方式：软件渲染而非 GL 互通

mpv 的 render API 支持把解码结果直接渲染到一块 CPU 内存（`MPV_RENDER_API_TYPE_SW`）。
我们用这条路，而不是共享 OpenGL 纹理给 wgpu：

- **不需要 GL/wgpu 互操作**，三大平台的窗口系统差异全部绕开；
- 每帧只多一次内存拷贝（1080p 约 8MB），换来三平台行为完全一致；
- 渲染目标尺寸 = 画面在窗口里实际占的**物理像素**，且不超过视频原始分辨率，
  等于把缩放做在 mpv 内部（swscale 质量好），GPU 侧只做 1:1 贴图。

代价是 4K/120fps 这类极端场景会被 CPU 拷贝拖住。真到那一步，可以按同样的接口
再加一个 D3D11/GL 零拷贝后端，UI 层不需要改。

## 构建

```bash
# 1. 准备 libmpv（放到 libs/<平台>/）
./scripts/fetch-libmpv.sh

# 2. 编译运行
cargo run --release -- path/to/video.mkv
```

### libmpv 从哪来

程序**不链接** libmpv，运行时按顺序查找（见 `src/player/ffi.rs`）：

1. 环境变量 `NGY_MPV_LIB` 指定的文件；
2. 可执行文件所在目录；
3. macOS 的 `Contents/Frameworks`；
4. `libs/<平台>/`（`windows-x86_64` / `macos-arm64` / `linux-x86_64` …）；
5. 系统加载器路径。

| 平台 | 库文件名 | 获取方式 |
| --- | --- | --- |
| Windows | `libmpv-2.dll` | 脚本自动下载 [zhongfly/mpv-winbuild](https://github.com/zhongfly/mpv-winbuild) 的 dev 包 |
| macOS | `libmpv.2.dylib` | `brew install mpv`，脚本会复制到 `libs/macos-*/` |
| Ubuntu | `libmpv.so.2` | `sudo apt install libmpv2`（或 `libmpv-dev`） |

库缺失时窗口照常打开，界面上会直接写明缺哪个文件、三条解决路径分别是什么——
而不是启动就崩。

## 用法

```
ngy-video-player [选项] [文件或目录...]

  -f, --fullscreen   以全屏启动
  -h, --help         帮助
  -V, --version      版本
```

拖入文件、目录、`.m3u`/`.m3u8`/`.pls` 播放列表都可以。目录会展开成其中的媒体文件，
按自然序排列（`第2集` 在 `第10集` 前面）。拖单个文件时同目录的其它媒体会自动排队。

### 快捷键

| 键 | 动作 |
| --- | --- |
| `空格` | 播放 / 暂停 |
| `←` `→` | 快退 / 快进 5 秒 |
| `↑` `↓` | 音量 ±5% |
| `N` / `P` | 下一个 / 上一个 |
| `[` `]` | 减速 / 加速 |
| `M` | 静音 |
| `L` | 播放列表 |
| `O` | 打开文件 |
| `F` / 双击画面 | 全屏 |
| `Esc` | 退出全屏 / 逐层关闭浮层（帮助 → 菜单 → 播放列表） |
| `?` | 快捷键帮助 |

单击画面本身是播放 / 暂停。控制条、菜单、播放列表这些浮层会把点击吃掉，
不会顺带把视频暂停——每个图标按钮和浮层容器都调用了 `stop_propagation`，
因为 GPUI 会把同一个点击派发给**同位置命中的全部 hitbox**，而不是只给最上层那一个。

### 设为默认视频播放器

菜单「设置 → 设为默认视频播放器…」会把本程序登记为 60 个格式（54 个视频容器 + 6 个
播放列表格式）的候选。**Windows 上这一步没法静默完成**：`UserChoice` 带哈希校验，程序
只能把入口放进「默认应用」列表，再由用户在打开的设置页里选定（见 `docs/adr/0002`）。

同一个菜单里还有「移除本程序的文件关联」用于撤销，「启动时自动维护关联」控制每次启动
是否按当前可执行文件的路径重写一遍——关掉之后，默认播放器被别的软件拿走也不会再改回来。

### 环境变量

| 变量 | 用途 |
| --- | --- |
| `NGY_MPV_LIB` | 指定 libmpv 库文件路径 |
| `NGY_MPV_VERBOSE=1` | 输出 libmpv 详细日志（排查解码问题） |
| `NGY_VIDEO_LOG_DIR` | 自定义日志目录 |
| `NGY_VIDEO_CONFIG_DIR` | 自定义配置目录 |

日志与配置位置：

- Windows：`%LOCALAPPDATA%\ngy-video-player\logs\`、`%APPDATA%\ngy-video-player\settings.json`
- macOS：`~/Library/Logs/ngy-video-player/`、`~/Library/Application Support/ngy-video-player/`
- Linux：`~/.local/state/ngy-video-player/`、`~/.config/ngy-video-player/`

## 代码结构

```
src/
├─ main.rs          入口：参数、帮助
├─ app.rs           主视图：窗口、快捷键、控制条、播放列表、OSD
├─ ui.rs            配色、图标按钮、时间格式、等比画面
├─ formats.rs       扩展名清单（文件过滤器 + 拖入判断）
├─ playlist.rs      播放列表与 m3u/pls 解析、目录展开、自然序
├─ settings.rs      偏好持久化
├─ logging.rs       终端 + 滚动日志文件
├─ platform/        系统集成
│  └─ associations.rs  文件关联：注册 / 撤销 / 状态（按平台分文件）
└─ player/
   ├─ ffi.rs        libmpv 的运行时绑定（dlopen，不用头文件）
   ├─ engine.rs     引擎线程 + 取帧线程 + 状态快照
   └─ mod.rs
```

线程模型：

- **引擎线程**：`mpv_wait_event` 循环，处理命令、属性变化、文件结束；
- **取帧线程**：持有 `mpv_render_context`（软件渲染），有帧就渲染到 64 字节对齐的
  缓冲，包装成 `RenderImage` 推给界面；
- **主线程**：GPUI 渲染。后台通过 `futures::mpsc` 单向推事件，
  `AsyncApp` 不是 `Send`，跨线程只能走这条通道。

## 已知限制

- 软件渲染路径在高分辨率 + 高帧率下受内存带宽限制；1080p60 无压力，4K120 需要
  硬件后端（接口已预留）。
- HDR（PQ/HLG）目前按 SDR 输出，颜色会偏；需要显式 tone-mapping 才能正确显示。
- 硬件解码默认关闭（`hwdec=no`）：软件渲染下 hwdec 要把帧从显存倒回内存，通常更慢。
  可在 `settings.json` 里改成 `auto-safe` 试。
