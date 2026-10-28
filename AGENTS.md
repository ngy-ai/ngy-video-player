# AGENTS.md

## 项目概述

跨平台视频播放器：**gpui-kit**（Zed GPUI）做界面，**libmpv** 做解复用/解码/字幕。
Windows / macOS / Ubuntu 同一套纯 Rust 代码，libmpv 通过 `libloading` 在运行时加载。

## 常用命令

```bash
cargo build                       # 构建（debug）
cargo run --release -- video.mkv  # 运行
cargo test                        # 全部测试
cargo test --test playlist        # 单个测试文件
cargo fmt                         # 格式化
cargo clippy --all-targets        # 静态检查
./scripts/fetch-libmpv.sh         # 拉取 libmpv 到 libs/<平台>/
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
                 picture、playlist_ops、video_area、controls、menus、bar、
                 menu_bar、associations、playlist_ui、overlays、panels、mini、render
src/ui.rs        配色、图标按钮、时间格式、等比画面
src/formats.rs   扩展名清单（文件过滤器 + 拖入判断）
src/playlist.rs  播放列表、m3u/pls 解析、目录展开、自然序
src/screenshot.rs 截图落点与文件名
src/settings.rs  偏好持久化
src/logging.rs   终端 + 滚动日志文件
src/platform/    associations.rs（文件关联：注册/撤销/状态，按平台实现）
src/player/      ffi.rs（运行时绑定）、frame.rs、probe.rs、thumbnail.rs
src/player/engine.rs  引擎入口：Engine 句柄、Shared 共享状态、命令/查询接口
src/player/engine/    state（对外类型）、event_loop、dispatch、render_loop
```

## 约定

- **线程模型不可随意改动**：mpv 事件循环和取帧在后台线程；GPUI 的 `AsyncApp`
  不是 `Send`，跨线程只能通过 `futures::mpsc` 把事件推回主线程。
- 播放器状态以 `engine.rs` 的状态快照为准，界面层不直接持有 mpv 句柄。
- 浮层/按钮的点击处理必须 `stop_propagation`，否则同位置的视频画面也会收到点击。
- 注释用中文，解释「为什么」而不是复述代码；错误信息要给出可执行的解决路径。
- 新增依赖前先确认必要性；`gpui-kit` 版本与其它 ngy-* 项目保持一致。
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
  - 单元测试就近放在源码文件内的 `#[cfg(test)] mod tests`；超过约 150 行就移到
    同目录的 `tests.rs`（`src/foo/tests.rs`），并在 `foo.rs` 里声明
    `#[cfg(test)] mod tests;`。
  - 跨模块的集成测试留在 `tests/` 顶层。

现状：`src/app.rs` 约 5300 行，是已知的超限文件。改动它时先把要动的区域
抽成子模块再加新逻辑，不要在这个文件里继续堆。

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
