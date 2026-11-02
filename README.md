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
# 1. 准备 libmpv 与 ffmpeg（放到 libs/<平台>/）
./scripts/fetch-libmpv.sh
./scripts/fetch-ffmpeg.sh

# 2. 编译运行
cargo run --release -- path/to/video.mkv
```

### 打包：一个 exe

release 构建会把 `libs/<平台>/` 里的 libmpv 与 ffmpeg **内嵌进可执行文件**，所以
`target/release/ngy-video-player.exe` 本身就是完整分发包：拷到任何一台没装过 mpv、
也没装 ffmpeg 的机器上都能直接播、直接裁切片段。

```bash
./scripts/fetch-libmpv.sh   # 库放到 libs/<平台>/（只需一次）
./scripts/fetch-ffmpeg.sh   # 可执行文件放到另一个目录（同样只需一次）
cargo build --release       # 产物约 220MB：约 116MB libmpv + 约 90MB ffmpeg
```

首次启动会把内置的库与 ffmpeg 解压到用户缓存目录，之后直接复用：

| 平台 | 落点 |
| --- | --- |
| Windows | `%LOCALAPPDATA%\ngy-video-player\libmpv\`、`...\ffmpeg\` |
| macOS | `~/Library/Caches/ngy-video-player/libmpv/`、`.../ffmpeg/` |
| Linux | `~/.cache/ngy-video-player/libmpv/`、`.../ffmpeg/` |

文件名带内容哈希（`9665c142b49567cc-libmpv-2.dll`）：换了版本会写出新文件，
旧的在下次解压后清掉，不会误用上一版的库。

exe 也不依赖系统里的 VC++ 运行库：Windows 目标在 `.cargo/config.toml` 里静态链接
MSVC 运行库（`+crt-static`），干净系统上双击就能开，不会弹出「由于找不到
VCRUNTIME140.dll，无法继续执行代码」（见 `docs/adr/0006`）。

- debug 构建不内嵌（开发期 `libs/` 就在手边，每次链接搬上百 MB 没有意义）；
- `NGY_EMBED_MPV=<路径>` 指定要内嵌的库，`NGY_EMBED_MPV=0` 关掉内嵌，
  做出「库放在旁边」的轻薄 exe；`NGY_EMBED_FFMPEG` 对 ffmpeg 同理；
- 内嵌只搬 libmpv 本身：Windows 的 `libmpv-2.dll` 是自包含的，macOS/Linux 从
  Homebrew / 发行版拿到的库还依赖同目录或固定路径上的其它共享库，跨机器分发请
  继续用 `bash scripts/bundle-macos.sh` 生成的 `.app`。

### libmpv 从哪来

程序**不链接** libmpv，运行时按顺序查找（见 `src/player/ffi.rs`）：

1. 环境变量 `NGY_MPV_LIB` 指定的文件；
2. 可执行文件所在目录；
3. macOS 的 `Contents/Frameworks`；
4. `libs/<平台>/`（`windows-x86_64` / `macos-arm64` / `linux-x86_64` …）；
5. 系统加载器路径；
6. 编译期内嵌的那一份（release 构建，见上节「打包：一个 exe」）。

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

拖入文件、目录、`.m3u`/`.m3u8`/`.pls` 播放列表都可以。目录会连同子文件夹一起展开成
树形列表：每层文件夹在前、按自然序排列（`第2集` 在 `第10集` 前面），点文件夹行展开或
收起，收起后里面的项也不再参与「下一集」。

拖动行可以调整顺序，也能搬家：落在文件夹上下两端是插到它前面/后面，落在中间是把这一项
（文件夹则连同里面的东西）移进那个文件夹。拖单个文件时同目录的其它媒体会自动排队。

页签上右键可以重命名或删除这一页列表；右上角的 × 是同一个删除，只剩一页时不让删。

标题栏的折叠按钮可以一键收起 / 展开所有文件夹（收起后显示成「展开」，排序菜单里也有
同样两项）。收起的文件夹里的项既不出现在列表上，也不参与「下一集」。

### 快捷键

| 键 | 动作 |
| --- | --- |
| `空格` | 播放 / 暂停 |
| `←` `→` | 快退 / 快进 5 秒 |
| `↑` `↓` | 音量 ±5% |
| 滚轮 | 音量 ±5% |
| `Ctrl`/`⌘` + 滚轮 | 逐帧：上滚后退 / 下滚前进（自动暂停） |
| `Ctrl`/`⌘` + `←` `→` | 逐帧后退 / 前进（自动暂停） |
| `N` / `P` | 下一个 / 上一个 |
| `[` `]` | 减速 / 加速 |
| `M` | 静音 |
| `S` | 截图 |
| `L` | 播放列表 |
| `O` | 打开文件 |
| `F` / 双击画面 | 全屏 |
| `A` | 打开设置（画面页） |
| `Esc` | 退出全屏 / 逐层关闭浮层（设置 → 菜单 → 播放列表） |
| `?` | 打开设置（快捷键页） |

播放菜单里有 A-B 循环：播到起点时点「标记 A-B 起点」，到终点时点「标记 A-B 终点」，
之后只反复播放这一段；换片或点「清除 A-B 循环」会取消。

单击画面本身是播放 / 暂停。控制条、菜单、播放列表这些浮层会把点击吃掉，
不会顺带把视频暂停——每个图标按钮和浮层容器都调用了 `stop_propagation`，
因为 GPUI 会把同一个点击派发给**同位置命中的全部 hitbox**，而不是只给最上层那一个。

截图（`S`）由 mpv 抓取解码后的原始画面，存到系统图片目录下的 `影匣截图/`
（Windows 是 `%USERPROFILE%\Pictures\影匣截图`，macOS/Linux 是 `~/Pictures/影匣截图`），
文件名形如 `视频名_01-23-45.png`，连拍多张也不会互相覆盖。截图失败只在 OSD 里说明原因，
不会打断播放。

存好之后画面左上角会浮出一张卡片：缩略图 +「复制 / 打开 / 定位」。复制在 Windows 与
macOS 上是把图片本身放进剪贴板，Linux 的两个后端只支持文本，那里复制的是文件路径
（按钮文案会写成「复制路径」）。鼠标停在卡片上时不自动收起，`Esc` 或右上角的 `×`
可以立刻关掉。

### 裁切片段

菜单「文件 → 裁切片段…」在右侧打开裁切面板（与播放列表共用同一条通道，两者互斥），
播放不会中断。开始、结束可以手动输入（`90`、`1:30`、`01:02:03.500`），也可以边播边定位：
拖主控制条的进度条、空格播放/暂停、←/→ 前后 5 秒、Ctrl/⌘+←/→ 逐帧（面板上也有
「上一帧 / 下一帧」），停在哪一帧就点「设为开始 / 设为结束」，取的是播放器当前时间。
「预览片段」会从开始播到结束并自动停下，方便用画面和声音整体确认。

- 开始留空 = 从头开始；
- 结束留空 = 直到最后；
- 两端都留空 = 不裁切，按钮不可用。

面板里可以选导出参数：

- **导出方式**：精确重编码（切点精确到帧）或无损流复制（不重新编码，几秒完成，
  切口落在前一个关键帧上，可能比设定时间早几秒）；
- **编码与画质**：H.264 / H.265，高画质 / 标准 / 小体积三档 CRF；
- **画面处理**：HDR → SDR 色调映射、去隔行、降噪、锐化、恒定帧率（只在重编码时可用）；
- **音频**：EBU R128 响度标准化（-16 LUFS）、±5 秒音频偏移（0.1 秒步进，修音画不同步）；
- **轨道**：音轨、字幕、章节与元数据是否保留。

默认是精确重编码为 H.264 + AAC 的 mkv，字幕与多条音轨一并保留；输出到原文件同目录，
名字是「原文件名 - 片段.mkv」，已有同名文件时自动编号，不会覆盖。耗时与片段长度、
CPU 相关，进度显示在面板里，可随时取消。

### 章节与元数据

菜单「文件 → 编辑章节与元数据…」打开编辑对话框：

- 标题 / 作者 / 备注直接改，留空表示保存时删除该项；
- 「在当前位置添加」以播放位置作为章节起点，标题可直接编辑，右侧 × 删除；
- 章节按起点自动排序；两条离得太近（小于 0.1 秒）会被合并。

保存走流复制：画面、音轨、字幕、附件都不重新编码，几秒结束；输出到同目录的
「原文件名 - 元数据.mkv」，原文件不动。内置字幕若是 mp4 的 mov_text，会转成 srt
才装得进 mkv；点阵字幕（PGS/DVD）与 ASS 原样保留。

输入框支持输入法与选区，中文标题可以直接打字。章节时间点目前只能取播放位置，
不支持手改；需要精确到帧时先定位再添加。

### 设置窗口

所有偏好都收在一个模态窗口里（菜单「设置 → 设置…」，或按 `A` / `?` 直达对应页），
左侧标签切换分组，改动立刻生效并落盘，没有「确定 / 取消」：

| 标签页 | 内容 |
| --- | --- |
| 画面设置 | 画面比例、旋转、镜像 |
| 视频设置 | 渲染模式、解码方式、播放显卡 |
| 播放设置 | 循环模式、窗口置顶策略 |
| 文件关联 | 设为默认播放器、撤销关联、启动时自动维护 |
| 快捷键设置 | 全部键位表 |

左列底部的「恢复默认设置」把这五页里的选项拉回出厂值；音量、窗口大小、面板宽度
这些「用着用着调出来的」值不受影响。

### 视频渲染与解码

「视频设置」页里三组单选，都随偏好保存：

- **视频渲染模式**：两档走的都是同一条软件渲染路径，区别只在内部渲染分辨率——
  性能模式把高度封顶在 720（全屏看高清时省下的缩放与回读是耗电大头），
  稳定模式按窗口实际尺寸渲染，画面更细腻。
- **视频解码模式**：分别对应 mpv 的 `auto-safe` / `auto-copy` / `no`。三档都用
  「拷回内存」的变体，因为主画面走软件渲染，零拷贝的硬件帧接不住。
- **播放显卡**：只在 Windows 上能列出（`EnumDisplayDevicesW`），写进 mpv 的
  `d3d11-adapter`；只影响硬件解码那一路，软件解码时与显卡无关。

### 设为默认视频播放器

设置窗口「文件关联」页的「设为默认视频播放器」会把本程序登记为 60 个格式
（54 个视频容器 + 6 个播放列表格式）的候选。三个平台的机制完全不同：

| 平台 | 落点 | 用户还要做什么 |
| --- | --- | --- |
| Windows | `HKCU\Software\Classes` + `Capabilities` + `RegisteredApplications` | 要。在打开的「默认应用」里选一下本程序 |
| Linux | `~/.local/share/applications/*.desktop` + `~/.config/mimeapps.list` | 不用，注册即生效 |
| macOS | `.app` 的 `Info.plist` + LaunchServices | 不用，注册即生效 |

**Windows 上这一步没法静默完成**：`UserChoice` 带哈希校验，程序只能把入口放进「默认
应用」列表，剩下那一下必须由用户亲手点（见 `docs/adr/0002`）。

**macOS 必须先打包**：`bash scripts/bundle-macos.sh` 生成 `dist/ngy-video-player.app`，
再从包里启动。裸二进制在 LaunchServices 眼里没有身份，注册会直接报错并给出这条指引。

同一页里还有「移除本程序的文件关联」用于撤销，「启动时自动维护文件关联」控制每次启动
是否按当前可执行文件的路径重写一遍——关掉之后，默认播放器被别的软件拿走也不会再改回来。

### 环境变量

| 变量 | 用途 |
| --- | --- |
| `NGY_MPV_LIB` | 指定 libmpv 库文件路径 |
| `NGY_FFMPEG` | 指定裁切用的 ffmpeg 可执行文件路径 |
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
│  └─ app/          各分区实现；settings_panel/ 是统一设置窗口（左侧标签分页）
├─ ui.rs            配色、图标按钮、时间格式、等比画面
├─ formats.rs       扩展名清单（文件过滤器 + 拖入判断）
├─ playlist.rs      播放列表：树形条目、多页签、存档
│  ├─ scan.rs       目录扫描、自然序、m3u/pls 解析
│  ├─ tree.rs       文件夹递归展开、折叠、拖放移动
│  └─ sort.rs       逐层排序与顺序历史
├─ screenshot.rs    截图落点与文件名
├─ clip.rs          裁切片段：时间解析、区间校验、ffmpeg 任务与进度
│  └─ export.rs     导出参数：无损剪切、编码、滤镜、响度、偏移、轨道保留
├─ metadata.rs      章节与元数据编辑：ffmetadata、流复制任务
├─ subtitle_codec.rs 字幕编码选择（轨道编辑与元数据编辑共用）
├─ embed.rs         内嵌资源（libmpv / ffmpeg）解压到缓存目录
├─ settings.rs      偏好持久化
├─ logging.rs       终端 + 滚动日志文件
├─ platform/        系统集成
│  ├─ ffmpeg.rs    裁切用的 ffmpeg：内嵌 → exe 同目录 → libs/ → PATH
│  └─ associations.rs  文件关联：注册 / 撤销 / 状态
│     ├─ windows.rs    注册表 + Capabilities
│     ├─ linux.rs      .desktop + mimeapps.list
│     ├─ macos.rs      LaunchServices
│     ├─ mimeapps.rs   Linux 的纯逻辑（跨平台可测）
│     └─ plist.rs      macOS 的 Info.plist 生成（跨平台可测）
└─ player/
   ├─ ffi.rs        libmpv 的运行时绑定（dlopen，不用头文件）
   ├─ embed.rs      release 内嵌的 libmpv：解压到缓存目录再加载
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
  需要的话在设置窗口的「视频设置」页里切到智能解码或硬件解码，不必手改配置文件。
- 播放显卡只在 Windows 上能列出（`EnumDisplayDevicesW`），且只对硬件解码这一路有意义；
  Linux/macOS 显示「系统自动选择」。
- 单文件分发的 exe 约 220MB（约 116MB libmpv + 约 90MB ffmpeg），首次启动要多花一次
  解压的时间写进缓存目录。不想要这个体积就 `NGY_EMBED_MPV=0 NGY_EMBED_FFMPEG=0
  cargo build --release`，改回「依赖放在 exe 旁边 / 用系统里的 ffmpeg」的分发方式。
- 裁切的 HDR → SDR 色调映射依赖 ffmpeg 的 `zscale` / `tonemap` 滤镜；换成精简版
  ffmpeg 时可能报错，取消勾选「HDR → SDR」或改用无损流复制即可。
- 元数据编辑固定输出 mkv：章节与字体附件在 mp4 里的写法各不相同，只维护一套落点。
  章节时间点只能按播放位置添加，不能手动输入时间码。

文件关联相关的限制：

- **不会自动抢默认**：Linux/macOS 上注册即生效，Windows 上只能把本程序送进「默认应用」
  列表，最后一步由用户点。
- **macOS 的「移除关联」只撤声明**：系统没有「把默认还回去」的接口，已经选定的默认要
  用户自己在 Finder 的「显示简介 → 打开方式」里改。
- **macOS 没有应用图标**：`.icns` 还没生成，Dock 里是通用图标。
- **Linux 依赖 shared-mime-info**：系统里没有 `/usr/share/mime/globs2` 时只认内置表里的
  二十来个常见格式，其余扩展名会被跳过而不是猜一个 MIME 蒙上去。
