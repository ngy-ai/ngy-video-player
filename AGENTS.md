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
src/app.rs       主视图：窗口、快捷键、控制条、播放列表、OSD
src/ui.rs        配色、图标按钮、时间格式、等比画面
src/formats.rs   扩展名清单（文件过滤器 + 拖入判断）
src/playlist.rs  播放列表、m3u/pls 解析、目录展开、自然序
src/screenshot.rs 截图落点与文件名
src/settings.rs  偏好持久化
src/logging.rs   终端 + 滚动日志文件
src/platform/    associations.rs（文件关联：注册/撤销/状态，按平台实现）
src/player/      ffi.rs（运行时绑定）、engine.rs（引擎/取帧线程）
```

## 约定

- **线程模型不可随意改动**：mpv 事件循环和取帧在后台线程；GPUI 的 `AsyncApp`
  不是 `Send`，跨线程只能通过 `futures::mpsc` 把事件推回主线程。
- 播放器状态以 `engine.rs` 的状态快照为准，界面层不直接持有 mpv 句柄。
- 浮层/按钮的点击处理必须 `stop_propagation`，否则同位置的视频画面也会收到点击。
- 注释用中文，解释「为什么」而不是复述代码；错误信息要给出可执行的解决路径。
- 新增依赖前先确认必要性；`gpui-kit` 版本与其它 ngy-* 项目保持一致。
- 不要提交大文件、密钥、日志或本地配置。

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
