# AGENTS.md

## 项目概述

跨平台**视频播放器 + 视频剪辑器**：**gpui-kit**（Zed GPUI）做界面，**libmpv** 做解复用/解码/字幕，
**ffmpeg** 做多轨合成与编码。Windows / macOS / Ubuntu 同一套纯 Rust 代码，libmpv 通过
`libloading` 在运行时加载。

剪辑的目标是**从零编排成片**：空工程 → 定序列规格 → 导入素材 → 放上轨道 → 剪接（插入/覆盖/
修剪/切割）→ 转场与效果 → 关键帧 → 音频与字幕 → 导出。功能差距、阶段任务与任务规则写在
`TODO.md`，验收判据是它的附录 A（剧本跑不通就不算剪辑器可用）。

裁切片段、格式转换、时间线导出都要调用 **ffmpeg** 重编码：release 构建把 ffmpeg 与 libmpv 一起
内嵌进 exe，运行时解压到缓存目录再以子进程调用（见 `src/platform/ffmpeg.rs`、`src/embed.rs`）。

## 常用命令

```bash
cargo build                       # 构建（debug）
cargo run --release -- video.mkv  # 运行
cargo test                        # 全部测试（不需要 ffmpeg）
cargo test --test playlist        # 单个测试文件
cargo test -- --ignored           # 端到端测试：需要本机 ffmpeg，默认忽略
cargo test --test stress          # 长片压测：2h/300 片段与 3h/4K/1000 片段的结构层预算
cargo fmt                         # 格式化
cargo clippy --all-targets        # 静态检查
./scripts/fetch-libmpv.sh         # 拉取 libmpv 到 libs/<平台>/
./scripts/fetch-ffmpeg.sh         # 拉取 ffmpeg 到 libs/<平台>/
```

- Rust edition 2024，MSRV 跟随当前稳定版。
- libmpv 缺失时程序仍可启动，只是无法播放；不要把它改成启动即崩溃。
- `libs/`、`.cache/`、`target/` 不入库，不要提交。
- 工程格式 v2 起是 `sequences` 列表 + `active_sequence` 指针：字段语义的搬运一律写在
  `project.rs::migrate`（缺省 `version` 按 v1 处理），只靠 `serde(default)` 会让老工程
  读出空序列列表；列表非空、id 唯一与指针有效由 `project/sequences.rs` 自愈。

## 代码结构

```
src/main.rs      入口：参数解析、帮助
src/app.rs       主视图入口：状态类型、PlayerApp 字段、快捷键、run()
src/app/         子模块（同一 PlayerApp 的多个 impl 块）：
                 session（初始化/事件循环）、playback、window（迷你窗/置顶）、
                 picture、playlist_ops、video_area、
                 timeline_view / timeline_tracks（工程模式时间线：布局、轨道行、片段与
                   关键帧标记的拖动，`timeline_tracks/lane.rs` 是片段区各自画什么
                   （含两端修剪抓手与按工具切换的光标），`timeline_tracks/drop_ops.rs`
                   是各类落点与松手语义（修剪工具 `TrimTool` 的 V/B/R/Y/U 与修饰键覆盖
                   都走 `effective_trim`），
                   `timeline_tracks/label.rs` 是轨道头与轨道右键面板的入口；
                   `timeline_view/` 下 `document` 是打开 / 新建 / 关闭、
                   `history_ops` 是撤销重做、`zoom` 是播放头与缩放、`render` 是整块绘制）、
                 trim_window（双画面修剪窗口：切点前后两帧 + 时间码，ffmpeg 单帧导出在
                   后台线程跑，松手后才取帧）、
                 timeline_preview（工程模式预览：取帧与跟随，`timeline_preview/composite.rs`
                   是多轨合成接管，`playback.rs` 是 J/K/L 变速、播放时钟与 scrub，
                   `audio.rs` 是每条音频轨一路纯音频 mpv 实例的多路出声）、
                 preview_proxy（预览取文件的唯一入口：读「预览用代理」开关、判断哪段在用代理、
                   覆盖写代理前把预览从该文件上撤下来；规则本身在 `timeline::proxy`）、
                 proxy_queue（批量补代理：素材库里「比序列大、还没有代理」的串行队列，
                   进度与取消在素材库底栏，换工程时整队作废）、
                 render_cache_queue（渲染缓存队列：把「有效果 / 调色 / 关键帧」的片段按导出
                   滤镜链渲成 `<素材>-<指纹>.cache.mp4`，一次一条，进度在设置窗口那一页）、
                 render_cache_queue（渲染缓存队列：把「有效果 / 调色 / 关键帧」的片段按导出
                   滤镜链渲成 `<素材>-<指纹>.cache.mp4`，一次一条，进度在设置窗口那一页）、
                 library（工程模式左侧素材库：导入 / 后台探测 / 封面 / 试听原片，
                   `library/drop.rs` 是拖到轨道的落点、兼容性与首尾相接排布，
                   `library/tags.rs` 是批量打标签与智能合集条）、
                 source_edit（三点 / 四点编辑：源入出点、`,` / `.` 落位，工作区两端都定了
                   走四点并把画面与声音分别排到目标轨，与试听分成两笔状态）、
                 types（界面自己的小状态类型：横幅、右键菜单、悬停预览、菜单栏条目、MiniState）、
                 history（工程模式右侧编辑历史侧栏）、
                 keys（动作 → 键位串，一处集中）、
                 timeline_effects（片段效果栈面板：加 / 开关 / 换档 / 调序 / 删）、
                 timeline_keys（片段关键帧面板：在播放头处记录与删除）、
                 timeline_speed（变速 / 定格面板：倍率档位与速度曲线，
                   `timeline_speed/curve.rs` 是曲线画布、可拖的倍率点与坐标换算）、
                 timeline_mask（片段遮罩面板：开关、矩形 / 椭圆、大小、羽化、位移）、
                 mixer_panel（混音台：每轨推子 / 声像条（按住拖动）、M / S、音量自动化点的
                   打 / 删 / 清空；推子走 dB 刻度，拖动状态在 `PlayerApp.mixer_dragging`，
                   拖动的像素换算在 `app/mixer_panel/tests.rs`）、
                 timeline_curve（曲线编辑器：网格与刻度、折线、贝塞尔手柄拖动、
                   框选删点、复制 / 粘贴曲线）、
                 timeline_subtitle（字幕编辑模态面板、字幕块右键菜单与预览入口，
                   `timeline_subtitle/file.rs` 是 SRT / VTT / ASS 导入导出、
                   `timeline_subtitle/karaoke.rs` 是可拖的逐字节拍条）、
                 subtitle_guides（工程预览上的动作 / 字幕安全框两圈参考线）、
                 sequence_manager（多序列管理面板：新建 / 切换 / 改名 / 删除与切换复位）、
                 timecode_jump（跳到时间码：Ctrl/⌘+G 打开、打开即聚焦、超长夹到结尾）、
                 controls、menus、bar、
                 menu_bar、associations、ab_loop（A-B 循环）、
                 project_file（工程保存/另存为/未保存确认）、
                 otio（导出 / 导入 `.otio`：导入新建一条序列，素材按 URL 登记）、
                 metadata（章节/元数据编辑，模态）、playlist_ui、
                 playlist_menu（面板上的弹出菜单：添加/排序/全部列表）、
                 playlist_sort（排序 + 拖动重排）、
                 clip/（裁切面板：状态、绘制、高级导出选项、批处理、预设，
                 与播放列表共用右侧通道）、
                 convert/（格式转换面板：同一条右侧通道，来源可选当前文件或文件夹，
                 目标格式是可搜索的下拉）、
                 remux（音轨/字幕编辑对话框）、
                 concat（拼接对话框）、proxy（代理生成对话框）、
                 multicam（双机位并排导出对话框）、scope（示波器）、
                 scenes（镜头分割）、overlays、mini、render、format、constants、
                 widgets（模态外壳、按钮、互斥 chip 等通用小件）、
                 settings_panel/（统一设置窗口：tabs + 各标签页 + 恢复默认；
                   `cache` 是渲染缓存的开关、上限与补缓存队列）
src/ui.rs        配色、图标按钮、时间格式、等比画面
src/formats.rs   扩展名清单（文件过滤器 + 拖入判断）
src/clip.rs      裁切片段：时间解析、区间校验、ffmpeg 任务与进度
src/clip/export.rs  导出参数：无损剪切、编码、滤镜、响度、音频偏移、轨道保留
src/clip/keyframe.rs  关键帧探测：流复制前把切点吸附到随机访问点
src/clip/concat.rs    拼接、cutout.rs（删除中间段 / 分割）、loudness.rs（两遍 loudnorm）
src/clip/batch.rs     批处理：把同一套参数套到整个文件夹
src/clip/preset.rs    导出预设的存盘与读取（presets.json）
src/encoder.rs   硬件编码器探测：NVENC / QSV / AMF / VideoToolbox
src/proxy.rs     代理文件生成：低码率预览副本
src/metadata.rs  章节与元数据编辑：ffmetadata、流复制任务
src/media.rs     素材探测：`ffmpeg -i` 解析时长/帧率/分辨率/色彩/声道并登记进工程
src/media_cache.rs  时间线媒体缓存入口：缓存根目录（`NGY_VIDEO_CACHE_DIR`）
src/media_cache/    key（路径哈希 + 大小 + mtime）、lru、thumbnail（100 秒一块）、waveform（流式峰值）
src/multicam.rs  双机位并排导出（sync 子模块：音频互相关自动对齐，依赖 rustfft）
src/subtitle_codec.rs 字幕编码选择（轨道编辑与元数据编辑共用）
src/convert.rs   格式转换：把当前文件或整个文件夹转成指定格式
src/convert/format.rs  目标格式表：覆盖 formats 里所有能写出的容器（封装器 + 编码参数）
src/convert/scan.rs    文件夹递归扫描（自然序、跳过符号链接）
src/convert/task.rs    批次任务：逐个文件重编码、输出命名、进度与取消
src/remux.rs     轨道编辑：换/删内置音轨、内嵌字幕（画面流复制，单路转码）与进度
src/embed.rs     内嵌资源（libmpv / ffmpeg）解压到缓存目录
src/playlist.rs  播放列表入口：item（一行）/ list（一份列表）/ set（多页列表）/ scan / sort / 	ree
src/playlist/scan.rs  目录扫描、自然序、m3u/pls 解析
src/playlist/tree.rs  文件夹递归展开、折叠、拖放移动（DropSpot）
src/playlist/sort.rs  逐层排序（名称/修改时间/添加时间）、最近 3 版顺序历史
src/screenshot.rs 截图落点与文件名
src/scope.rs     示波器：亮度直方图、波形图、矢量图的统计与位图
src/scenes.rs    镜头分割：ffmpeg 场景检测、切点解析、进度与取消
src/settings.rs  偏好持久化
src/logging.rs   终端 + 滚动日志文件
src/platform/    ffmpeg.rs（裁切用的 ffmpeg：内嵌→exe 同目录→libs→PATH）、
                 associations.rs（文件关联：注册/撤销/状态，按平台实现）、
                 pictures.rs（Windows「图片」库的真实位置，走 known folder，别拼 USERPROFILE）
src/player/      ffi.rs（运行时绑定）、embed.rs（内嵌库解压）、frame.rs、probe.rs、thumbnail.rs
src/player/compositor.rs  多轨软件合成入口：handle（界面侧句柄）/ layer（一路 mpv）/ worker（合成线程）
src/player/engine.rs  引擎入口：Engine 句柄、Shared 共享状态、命令/查询接口
src/player/engine/    state（对外类型）、event_loop、dispatch、render_loop
src/timeline.rs  剪辑时间线：入口只放文档、模块声明与 `pub use`
src/timeline/time.rs  Tick（i64 微秒）与 Rational 帧率：全项目唯一的帧↔时间换算入口
src/timeline/spec.rs  序列规格：分辨率预设、帧率表、奇数边校验、按素材推导
src/timeline/timecode.rs  `HH:MM:SS:FF` 的显示与解析：整帧 / 丢帧（29.97、59.94）、省略短写
src/timeline/media.rs  素材元信息、文件指纹与过期判断；不含 ffmpeg 调用
src/timeline/remap.rs  速度曲线（时间重映射）：素材位置做自变量、∫ds/rate 求时长与反解，
                 切片（`slice`，切开与修剪用）；`remap/ffmpeg.rs` 是翻成 ffmpeg 参数的那一侧
                 （`setpts` 表达式 + 音频分段 atempo；与 `timeline/speed.rs` 的改档位不是一回事）
src/timeline/mask.rs  片段遮罩：相对坐标的形状、羽化与位移换算、退化判定，以及导出用的
                 alpha 图（`alpha_at` / `alpha_plane` / `pgm`：有符号距离 + 羽化过渡）
src/timeline/mixer.rs  混音：三层音量（片段 → 自动化 → 推子）定序、等功率声像、
                 自动化到 ffmpeg `volume` 表达式；单测在 `mixer/tests.rs`
src/timeline/query.rs  素材搜索与智能合集：条件语法（七种字段、否定、并且）、
                 求值与 `Collection`（存条件不存结果），单测在 `query/tests.rs`
src/timeline/proxy.rs  代理选用规则：素材比序列大且代理在 → 预览用代理；导出拒绝代理素材
src/timeline/model.rs  序列 → 轨道 → 片段、转场与标记；纯数据，不持有文件路径
src/timeline/ops.rs    编辑操作纯函数：旧序列进、新序列出，非法操作返回错误
src/timeline/ops/placement.rs  放置语义：严格插入 / 插入让位 / 覆盖清区（`shift_open`、`clear_range`）
src/timeline/ops/edges.rs      边界编辑：普通 / 波纹修剪、切开、滑移 / 滑动 / 卷动；
                 `cut_at` 是「时间线位置 → 素材位置 + 两侧曲线切片」的唯一入口
                 （挂了速度曲线的片段靠它算时长，滑移 / 卷动这类 1:1 的操作直接拒绝）
src/timeline/ops/remove.rs     删除：留空隙的普通删除与波纹删除
src/timeline/ops/params.rs     片段参数：倍率、速度曲线、片尾定格、调色
src/timeline/ops/subtitle.rs   字幕条目增删改：未锁定的字幕轨 + 同轨不重叠
src/timeline/project.rs  .ngyproj 工程文件：版本迁移、相对素材路径、原子写 + .bak
src/timeline/project/sequences.rs  多序列：列表 + 当前指针、增删改切换、读盘自愈（ADR 0009）
docs/adr/0009  工程持有多条序列、界面状态仍只有一份
src/timeline/three_point.rs  三点 / 四点编辑的时间换算：源入点 + 源出点 + 时间线入点（有四点时按短边裁剪）
src/timeline/targets.rs  轨道目标：素材的音 / 视频分别落到哪条轨（首选 → 同类第一条没锁的）
src/timeline/transition.rs  转场编辑：两侧余量、加 / 改 / 去，时长按余量预钳（导出侧再兜底）
src/timeline/effect.rs  片段效果栈：去隔行 / 降噪 / 锐化 / 防抖 / 插帧 / 水印的滤镜与增删改
src/timeline/keyframe.rs  片段关键帧：缩放 / 位置 / 不透明度的曲线、求值与 ffmpeg 表达式
                 （缓动含自定义贝塞尔；导出侧按 8 段折线采样，单测在 `keyframe/tests.rs`）
src/timeline/color.rs  片段调色：亮度 / 对比度 / 饱和度 / 色温与 3D LUT 的滤镜映射
src/timeline/subtitle.rs  字幕条目、样式与逐字权重、预览取字；`subtitle/` 下 `ass.rs` 写 ASS 脚本
                 （含 `\kf`）、`ass_import.rs` 读 ASS、`text.rs` 读写 SRT / VTT、
                 `karaoke.rs` 算逐字节拍边界与厘秒分配
src/timeline/otio.rs  OpenTimelineIO 交换：export（序列 → JSON）、import（JSON → 中间结构）
src/timeline/cache.rs  预览渲染缓存：指纹、落点、LRU 清理计划、单片段渲染计划
src/timeline/cache.rs  预览渲染缓存：指纹、落点、LRU 清理计划、单片段渲染计划
src/timeline/export.rs  序列导出：`ExportPlan`、`ExportSettings`、字幕与遮罩附件、`render-presets.json` 预设
src/timeline/export/plan.rs  计划的组装：输入表、按范围切一刀、把几条流接起来（含黑底 + overlay）
src/timeline/export/plan/video.rs  单条视频流的滤镜链：效果 / 调色 / 关键帧 / 遮罩（alphamerge）/ xfade
src/timeline/export/plan/audio.rs  单条音频流的滤镜链：速度曲线按折点分段 atempo、段间 concat
src/timeline/export/stems.rs  分轨导出（ADR 0011 的替代路径）：每条听得见的音频轨一个 24bit wav，
                 长度等于序列，复用 `audio_stream` 的链（推子 / 自动化 / 声像都在里面）
src/app/stems.rs  「文件 > 导出音频分轨…」：选目录、`StemExport` 队列 + 代次、顺序跑
                 ExportTask、预览区底部横幅显示进度条与取消
src/export/task.rs  导出进程：临时 ASS 与遮罩 PGM 落盘 / 清理、软字幕按容器选编码器、进度与取消
src/timeline/undo.rs    撤销 / 重做：状态快照栈，一次操作一步，上限 200 步
src/timeline/autosave.rs  自动保存（30 秒无操作）与 `.autosave` 崩溃恢复
src/timeline/view.rs    时间线视图状态：缩放（1 像素 = 1 帧 ~ 10 秒）、滚动、标尺换挡
```

## 约定

- **线程模型不可随意改动**：mpv 事件循环和取帧在后台线程；GPUI 的 `AsyncApp`
  不是 `Send`，跨线程只能通过 `futures::mpsc` 把事件推回主线程。
- 播放器状态以 `engine.rs` 的状态快照为准，界面层不直接持有 mpv 句柄。
- 浮层/按钮的点击处理必须 `stop_propagation`，否则同位置的视频画面也会收到点击。
- 右侧只有一条通道：裁切、格式转换、播放列表三者互斥，宽度统一由
  `side_panel_width()` 算。新开一个这样的面板时，要同时收掉另外两个，并在
  `render()` 里把播放列表的显示条件补上它。全屏时这条通道整条让给画面
  （`side_panel_width()` 返回 0，`render()` 里那几个面板都不画），
  新面板同样要放进 `!fullscreen` 的分支里。
- 工程模式（`project.is_some()`）是另一套布局：右侧通道、控制条与进度条都不画，
  画面底部让出时间线（`timeline_height`），画面左右再分别让开素材库（`library_width()`）
  与编辑历史（`side_panel_width()`），两侧面板都只占预览区、不压时间线。
  新加面板/浮层时先想清楚它在工程模式下该不该出现；画面区与渲染目标（`update_render_target`）
  都要扣掉对应的宽度，漏一处就是画面被盖住或被拉伸。
- 模态对话框彼此互斥：新增一个对话框时，在它的 `open_*_dialog` 里把其它对话框
  都 `close_*_dialog` 一遍，并在 `render()` 里把它的 `render_*_dialog` 挂进模态层；
  漏掉的话两个模态会叠在一起抢键盘。
- 需要 ffmpeg 的端到端测试一律标 `#[ignore]`：没装 ffmpeg 的机器上 `cargo test`
  要能全绿，本地用 `cargo test -- --ignored` 手工跑。
- 注释用中文，解释「为什么」而不是复述代码；错误信息要给出可执行的解决路径。
- 新增依赖前先确认必要性；`gpui-kit` 已升到 0.9（2026-11），其它 ngy-* 项目如果还锁在
  0.6.x，同一个 target 目录里会各自编译一整套 gpui，升级尽量一起动。
- `.cargo/config.toml` 里的 `+crt-static` 不能删：exe 要能在没装 VC++ 运行库的
  干净 Windows 上直接跑（见 `docs/adr/0006`）。
- 不要提交大文件、密钥、日志或本地配置。

## 源码文件大小与拆分

大文件既难阅读也难修改，工具读一次就吃掉上下文。

- **单个源码文件不得超过约 700 行**（含空行与注释）。
- 超过就按职责拆成子模块，不要继续往上堆：新增代码时如果目标文件已经接近这个上限，
  先拆再改。拆分单独提交一次（`refactor:`），不要和功能改动混在同一个 commit。
- 拆分沿用现有惯例：
  - `src/foo.rs` 改成 `src/foo.rs`（只留模块文档、`mod` 声明和 `pub use` 重新导出）
    加若干 `src/foo/*.rs`。新模块一律用 `foo.rs` + `foo/` 布局，不再新增 `mod.rs`
    （`src/platform/mod.rs`、`src/player/mod.rs` 是历史写法，保持不动）。
  - 入口文件只放文档、导入和对外出口；共享类型与配置结构单独成文件
    （`foo/types.rs`、`foo/config.rs`，含默认值与校验）。
  - 实现按主题分文件：按职责命名的主题模块 + 入口里的组装逻辑；
    同一功能可以分散在多个文件里，不要为了「一个文件看全」把逻辑揉回去。
  - 可见性收窄：子模块默认私有，只有跨文件使用的项才 `pub(crate)`，
    真正的对外 API 由入口文件的 `pub use` 给出；实现细节不跨模块暴露。
  - 一个职责一个模块，依赖方向单一：底层模块（`player/`、`platform/`、`formats`）
    不许 import `app`/`ui`；需要回传时走事件或 `futures::mpsc`，不要反向依赖调用方。
  - 共享类型下沉到独立模块，不要让两个业务模块互相 import 形成环。
- 测试同样拆分，并镜像源码布局：
  - 集成测试 `tests/foo.rs` 拆成 `tests/foo.rs`（只留 `mod` 声明）
    + `tests/foo/<主题>.rs`。Cargo 只把 `tests/` 顶层文件当测试目标，
    子模块文件本身不是独立目标，所以子模块不加 `_test` 后缀，
    顶层文件名保持与源码模块同名（`playlist.rs` 对应 `src/playlist.rs`）。
    注意 `tests/foo.rs` 是 crate root，`mod` 默认只在 `tests/` 同层找文件，
    必须写成 `#[path = "foo/<主题>.rs"] mod <主题>;`（见 `tests/playlist.rs`）。
    需要本机 ffmpeg 的端到端用例就近放在对应主题文件里、标 `#[ignore]`
    （如 `tests/timeline_export/mask.rs`、`tests/timeline_export/remap.rs`）。
  - 单元测试就近放在源码文件内的 `#[cfg(test)] mod tests`；超过约 150 行就移到
    同目录的 `tests.rs`（`src/foo/tests.rs`），并在 `foo.rs` 里声明
    `#[cfg(test)] mod tests;`。
  - 跨模块的集成测试留在 `tests/` 顶层。

现状（2026-11-12 用 `[System.IO.File]::ReadAllText($f).Split("`n").Count - 1` 数过；
`(Get-Content $f).Count` 与 `Measure-Object -Line` 都少数几十到上百行，别再用它们）：
**现在都在 700 行以内**：最近拆的是 `app/bar.rs`（702 → 入口 + `bar/entries.rs`：
每套菜单的条目搬出去，入口只留「按当前模式分发」与动作派发）。
再往前拆过的：`player/compositor.rs`（753 → 入口 +
`handle` / `layer` / `worker`）、`playlist.rs`（748 → 入口 + `item` / `list` / `set`）、
`app/overlays.rs`（705 → 入口 + `hud` / `cards` / `menus` / `file_info`）。
再往前拆过的：`app.rs`（870 → 入口 + `app/types.rs`）、
`app/timeline_preview.rs`（751 → 入口 + `composite` / `playback` / `audio`）、
`app/timeline_menu.rs`（802 → 入口 + `clip` / `ruler` / `panels`）、`timeline/preview.rs`
（777 → 入口 + `audio`）、`timeline/model.rs`（749 → 入口 + `track`）、
`timeline/export/tests.rs`（775 → 入口 + `tests/audio.rs`）。
`src/timeline/ops/tests.rs` 已经从 735 拆到 656（放置 / 参数 / 字幕 / 关键帧 / 删除各一个文件）。
`src/timeline/ops.rs` 已经拆过（放置 / 参数 / 字幕各成子模块，入口约 600 行），可以直接照这个
样子拆：入口只留类型、`apply` 分发与跨模块辅助函数，其余按职责分文件。
新增代码时如果目标文件接近上限，先把要动的区域抽成子模块再加逻辑，不要继续往上堆。

## 任务收尾：必须更新 CURRENT_STATE.md

**每个任务完成后，开发 AI 必须更新 [CURRENT_STATE.md](CURRENT_STATE.md)。**

这是给下一次进场的 AI 看的：下次启动新任务，新的 AI 一进来读 CURRENT_STATE 就能续上，不用再口头解释一遍。

更新要求：

- 更新顶部日期。
- 修改“一句话状态”，保持为当前真实状态，不写流水账。
- 更新“已验证 / 尚未验证”，明确哪些是实际跑过的，哪些只是推断。
- 如果发现新的需求差距、坑或约束，写入对应章节。
- 更新“下一步建议”，让下一位接手者知道先做什么。
- 功能行为变化时，同步更新 README、需求文档、部署文档中不一致的描述。

任务没有验证完，不得在 CURRENT_STATE 里写成已完成。

## 完成后必须自动提交（强制规则）

**每次任务（无论大小）处理完成后，Agent 必须自动执行 git commit，无需用户再次确认。**

流程：

1. 确认代码可编译，运行相关测试；能过则运行 `cargo fmt` 与 `cargo clippy --all-targets`。
2. `git status` / `git diff` 检查改动，确认没有误改、没有密钥或大文件。
3. 只暂存本次任务相关的文件（`git add <文件>`，必要时再 `git add -A`）；
   不要把无关改动混进同一个提交。
4. 提交信息用中文，格式：`<类型>: <一句话说明>`，类型取
   `feat` / `fix` / `refactor` / `perf` / `docs` / `test` / `chore` / `build`。
   例如：`fix: 修复全屏切换后控制条不隐藏`。
5. 若目录尚未初始化 git 仓库，先执行 `git init` 再提交。
6. 如果测试失败或构建不通过，先修复；确实无法修复时**不要提交**，在回复中说明原因。
7. 提交后回复中给出 commit hash 与一句话摘要。

除上述自动提交外，不要执行 `git push`、改写历史、`--force` 或修改 git 配置，
除非用户明确要求。
