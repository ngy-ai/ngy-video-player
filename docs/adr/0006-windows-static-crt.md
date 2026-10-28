# Windows 静态链接 CRT，exe 不依赖 VC++ 运行库

`.cargo/config.toml` 给 MSVC 目标加了 `-C target-feature=+crt-static`，等价于 C/C++
项目里的「多线程 (/MT)」。

Rust 的 MSVC 目标默认动态链接 CRT，exe 的导入表里因此有 `vcruntime140.dll`。干净的
Windows（LTSC、刚装完的系统、精简版）没装 VC++ 运行库，双击就是系统弹窗「由于找不到
VCRUNTIME140.dll，无法继续执行代码」。

关键在于这个依赖**不在我们的代码里**：它是加载器在进程启动前解析导入表时用的，
内嵌一份 dll、运行时解压、改环境变量都帮不上忙——进程根本没起来，`main` 一行都没跑。
唯一的办法是链接期就不产生这个依赖。

替代方案是随包附带 `vcruntime140.dll` / `msvcp140.dll`（app-local 部署）：exe 体积
不变，但把「一个 exe」变回「一个目录」，而且这些 dll 必须和 exe 同目录才按名字
解析得到（DLL 搜索顺序），与 `docs/adr/0005` 的目标直接冲突。

代价与注意事项：

- exe 大几百 KB；
- 用 `cc` 编译的 C 依赖会跟着走 /MT：cargo 把 `crt-static` 作为 target feature 透给
  构建脚本，`cc` crate 据此选 /MT；
- 外部设置了 `RUSTFLAGS` 时，cargo 会忽略 `.cargo/config.toml` 里的 rustflags，
  CI 需要自己把 `+crt-static` 带上。

`libmpv-2.dll` 与这件事无关：`scripts/fetch-libmpv.sh` 拉的 zhongfly 构建是 MinGW
全静态，导入表里没有 vcruntime / msvcp，加载时不需要额外运行库。
