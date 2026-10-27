# 文件关联按平台各自原生实现

三个平台注册处理程序的方式不可通约：Windows 写 `HKCU\Software\Classes` 与 `Capabilities`，Linux 写 `.desktop` 与 `mimeapps.list`，macOS 必须先有 `.app` bundle 再用 LaunchServices 声明 `CFBundleDocumentTypes`。我们选择按 `cfg` 分派、各平台写原生绑定，而不是引入 `winreg` / `objc2` / `xdg-mime` 这类跨平台 crate：项目已经用手写 FFI 绑 libmpv，零新增依赖能让三个平台的实现保持同一副面孔，也免得为了写几次注册表键就扛进整套 objc 运行时绑定。

## Consequences

三个平台之间无法互相复用，改动不会自动对齐。注册项一律记录"当前可执行文件的绝对路径"，因此程序被移动或重新构建后必须重新注册一次，否则关联指向空气。
