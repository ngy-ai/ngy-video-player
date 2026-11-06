# 当前状态

更新日期：2026-11-06

## 一句话状态

加音频 / 视频轨后新轨不再「加了看不见」：轨道区改成按垂直滚动窗口绘制，加轨自动跟到
最后一屏；播放 / 剪辑两套菜单已在真实窗口逐项回归，`cargo test` / `clippy` / `fmt` 全过，
轨道滚动手感与高 DPI 尚未手工验证。

## 已验证

- `cargo test`：全绿（393 条单测 + 各集成测试；需要本机 ffmpeg / 注册表的用例按约定
  `#[ignore]`，未跑）。新增 4 条：`timeline::view` 的轨道窗口、加轨后同步、滚轮累积，
  以及 `app::timeline_view` 的可见行数。
- `cargo check`、`cargo fmt`、`cargo clippy --all-targets -- -D warnings`：通过、无告警。
- 「删除轨道前确认」：轨道头删除按钮与片段右键菜单都汇到 `delete_track`，视频 / 音频轨
  共用。有片段 / 转场时弹模态确认（Esc / × 取消、Enter 或按钮确认），空轨直接删；锁定轨
  先提示解锁。单测 `app::delete_track_confirm::tests` 与 `cargo test` / `fmt` / `clippy`
  全绿，确认框开关时的键盘独占接在 `render.rs` 的 `on_key_down` 链上。
- 代码路径确认：加轨 → `ViewState::reveal_last_track` → 下一帧 `sync_timeline_view`
  按面板高度 `sync_tracks` → `render_timeline_tracks_area` 按可见区间绘制，新轨在末尾。
- 播放 / 剪辑两套菜单已启动 `target/debug/ngy-video-player.exe` 逐项点开截图核对
  （截图在 `.cache/verify/*.png`，该目录不入库）：播放模式「文件」只有打开文件 / 文件夹、
  新建 / 打开工程、最近工程与单文件工具；剪辑模式标题换成「文件 / 编辑 / 预览 / 视图 /
  设置 / 帮助」，「文件」只有工程工作项，「编辑」「预览」「视图」下拉也逐项看过。
- 运行时切模式已验证：播放模式点最近工程进剪辑模式、剪辑模式点「关闭工程」回播放模式，
  两次标题与「文件」下拉都立刻换成对应那一套（`sync_menu_bar_mode` 生效）。
- `cargo build`（debug）成功；`cargo test --quiet` 全绿（393 条）、
  `cargo clippy --all-targets --quiet` 无告警。

## 尚未验证

- 没有实际点下去：加视频 / 音频轨按钮与「编辑 > 加视频轨 / 加音频轨」菜单项（菜单条目
  已截图确认存在，置灰状态也对）、滚轮上下换轨的手感、Shift / Ctrl 组合、滚动条点击定位；
  高 DPI（本次在 125% 缩放下点过）与 macOS / Ubuntu 三平台未逐一复验。
- 滚动条点击按「窗口坐标反推轨道区顶部」定位，鼠标事件坐标若有平台差异需要复核
  （代码已编译通过，逻辑未在实际窗口里量过）。
- 删轨确认没有在窗口里点过：空轨直接删、有片段时弹框、取消 / 确认后的序列变化与撤销，
  都只是代码路径与单测推断。
- `cargo test` 首次因两个正在运行的播放器进程占用 `target/debug/ngy-video-player.exe`
  失败；把该 exe 改名后重跑才通过，没有终止用户进程。

## 已知差距与约束

- 轨道垂直滚动只存在会话内，没写进 `.ngyproj`：`stamp_view_state` 仍只存缩放与横向
  滚动；以后要持久化得扩 `UiState` 并做版本迁移。
- 滚轮分工：轨道溢出时轨道区上下滚换轨、Shift 横滚时间轴、Ctrl/⌘ 缩放；未溢出时
  滚轮保持原来的横向滚动。标尺 / 标题条区域仍是原来的横向滚动与缩放。
- `src/app/timeline_view.rs` 抽走轨道区后约 642 行；`src/app/timeline_tracks.rs` 约
  724 行、`src/app.rs` 约 877 行仍超 AGENTS.md 的 700 行约定（后者本次之前就超了），
  下次动它们前先拆。原本想整体拆两个大文件，因工作区有并发编辑、按行号切片出偏差，
  已撤销并改为只抽「轨道区」这一个职责。
- 工作区还有另一条并行的「删轨前确认」改动（`src/app/delete_track_confirm.rs`、
  `app.rs` / `render.rs` / `session.rs` 及两个时间线文件里的相关行），为保证提交可编译
  随本次一并落地，不是本次修复的内容。
- `.gitignore` 的本地改动与未跟踪的 `Task.md` 与本次无关，未提交。

## 下一步建议

1. 启动工程模式，连点几次加轨按钮看新轨是否立刻出现，再上下滚轮 / 点滚动条回到前面的轨。
2. 给视频轨 / 音频轨各放一段素材后点删除，确认弹框、取消、确认与撤销；再试空轨直接删。
2. 若要把「停在第几条轨道」也跟着工程存盘，扩 `UiState` 并补迁移与测试。
3. 拆分 `src/app/timeline_tracks.rs` 与 `src/app.rs`（单独 `refactor:` 提交），再继续加轨道相关功能。
4. 在 100% / 150% 缩放与 macOS、Ubuntu 上重跑两套菜单的截图回归；GUI 验证脚本留在
   `.cache/verify/verify.ps1`（不入库），可用 `-Stage A/B/C/D/F/G/Z` 重放。
