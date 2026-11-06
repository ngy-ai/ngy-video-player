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
cargo fmt                         # 格式化
cargo clippy --all-targets        # 静态检查
./scripts/fetch-libmpv.sh         # 拉取 libmpv 到 libs/<平台>/
./scripts/fetch-ffmpeg.sh         # 拉取 ffmpeg 到 libs/<平台>/
```

- Rust edition 2024，MSRV 跟随当前稳定版。
- libmpv 缺失时程序仍可启动，只是无法播放；不要把它改成启动即崩溃。
- `libs/`、`.cache/`、`target/` 不入库，不要提交。

## 代码结构

```
src/main.rs      入口：参数解析、帮助
src/app.rs       主视图入口：状态类型、PlayerApp 字段、快捷键、run()
src/app/         子模块（同一 PlayerApp 的多个 impl 块）：
                 session（初始化/事件循环）、playback、window（迷你窗/置顶）、
                 picture、playlist_ops、video_area、
                 timeline_view / timeline_tracks（工程模式时间线：布局、轨道行与拖动）、
                 library（工程模式左侧素材库：导入 / 后台探测 / 封面 / 试听原片，
                   `library/drop.rs` 是拖到轨道的落点、兼容性与首尾相接排布）、
                 source_edit（三点编辑：源入出点、`,` / `.` 落位，与试听分成两笔状态）、
                 history（工程模式右侧编辑历史侧栏）、
                 keys（动作 → 键位串，一处集中）、
                 controls、menus、bar、
                 menu_bar、associations、ab_loop（A-B 循环）、
                 project_file（工程保存/另存为/未保存确认）、
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
                 settings_panel/（统一设置窗口：tabs + 各标签页 + 恢复默认）
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
src/playlist.rs  播放列表：树形条目（扁平数组 + depth）、多页签、存档
src/playlist/scan.rs  目录扫描、自然序、m3u/pls 解析
src/playlist/tree.rs  文件夹递归展开、折叠、拖放移动（DropSpot）
src/playlist/sort.rs  逐层排序（名称/修改时间/添加时间）、最近 3 版顺序历史
src/screenshot.rs 截图落点与文件名
src/scope.rs     示波器：亮度直方图、波形图、矢量图的统计与位图
src/scenes.rs    镜头分割：ffmpeg 场景检测、切点解析、进度与取消
src/settings.rs  偏好持久化
src/logging.rs   终端 + 滚动日志文件
src/platform/    ffmpeg.rs（裁切用的 ffmpeg：内嵌→exe 同目录→libs→PATH）、
                 associations.rs（文件关联：注册/撤销/状态，按平台实现）
src/player/      ffi.rs（运行时绑定）、embed.rs（内嵌库解压）、frame.rs、probe.rs、thumbnail.rs
src/player/engine.rs  引擎入口：Engine 句柄、Shared 共享状态、命令/查询接口
src/player/engine/    state（对外类型）、event_loop、dispatch、render_loop
src/timeline.rs  剪辑时间线：入口只放文档、模块声明与 `pub use`
src/timeline/time.rs  Tick（i64 微秒）与 Rational 帧率：全项目唯一的帧↔时间换算入口
src/timeline/spec.rs  序列规格：分辨率预设、帧率表、奇数边校验、按素材推导
src/timeline/media.rs  素材元信息、文件指纹与过期判断；不含 ffmpeg 调用
src/timeline/model.rs  序列 → 轨道 → 片段、转场与标记；纯数据，不持有文件路径
src/timeline/ops.rs    编辑操作纯函数：旧序列进、新序列出，非法操作返回错误
src/timeline/project.rs  .ngyproj 工程文件：版本迁移、相对素材路径、原子写 + .bak
src/timeline/three_point.rs  三点编辑的时间换算：源入点 + 源出点 + 时间线入点，缺哪个退化成两点
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
  - 单元测试就近放在源码文件内的 `#[cfg(test)] mod tests`；超过约 150 行就移到
    同目录的 `tests.rs`（`src/foo/tests.rs`），并在 `foo.rs` 里声明
    `#[cfg(test)] mod tests;`。
  - 跨模块的集成测试留在 `tests/` 顶层。

现状：所有源码文件都在 700 行以内（最大的三个：`src/app/convert/panel.rs` 约 673 行、
`src/app.rs` 约 655 行、`src/app/timeline_tracks.rs` 约 615 行）。
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
