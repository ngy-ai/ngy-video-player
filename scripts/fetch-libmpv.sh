#!/usr/bin/env bash
# 准备当前平台的 libmpv。
#
# 目的只有一个：让 `cargo run` 能直接找到动态库。程序本身不链接 libmpv，
# 运行时按「exe 目录 → libs/<平台>/ → 系统路径」的顺序查找（见 src/player/ffi.rs）。
# 另外 release 构建会把这个目录里的库内嵌进 exe，分发时只需要一个文件；
# 想要单文件产物，就先跑这个脚本再 cargo build --release。
#
# 用法：
#   ./scripts/fetch-libmpv.sh            # 准备到 libs/<平台>/
#   ./scripts/fetch-libmpv.sh --system   # 只检查系统里有没有，不复制
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*)
    platform="windows-x86_64"
    [ "$(uname -m)" = "aarch64" ] && platform="windows-arm64"
    libname="libmpv-2.dll"
    ;;
  Darwin)
    platform="macos-arm64"
    [ "$(uname -m)" = "x86_64" ] && platform="macos-x86_64"
    libname="libmpv.2.dylib"
    ;;
  Linux)
    platform="linux-x86_64"
    [ "$(uname -m)" = "aarch64" ] && platform="linux-arm64"
    libname="libmpv.so.2"
    ;;
  *)
    echo "不支持的平台: $(uname -s)" >&2
    exit 1
    ;;
esac

dest="$root/libs/$platform"
mkdir -p "$dest"

have_lib() {
  [ -f "$dest/$libname" ] && return 0
  for candidate in \
    "/opt/homebrew/lib/$libname" \
    "/usr/local/lib/$libname" \
    "/usr/lib/x86_64-linux-gnu/$libname" \
    "/usr/lib/aarch64-linux-gnu/$libname" \
    "/usr/lib64/$libname" \
    ; do
    [ -f "$candidate" ] && echo "$candidate" && return 0
  done
  return 1
}

if found=$(have_lib); then
  if [ "$found" != "$dest/$libname" ]; then
    echo "复制 $found → $dest/$libname"
    cp -L "$found" "$dest/$libname"
  else
    echo "已存在: $dest/$libname"
  fi
  exit 0
fi

if [ "${1:-}" = "--system" ]; then
  echo "系统里没有找到 $libname" >&2
  exit 1
fi

case "$platform" in
  windows-*)
    # zhongfly/mpv-winbuild 的 dev 包：只有 libmpv-2.dll + 头文件，体积比完整版小。
    archive="$root/.cache/mpv-dev.7z"
    mkdir -p "$root/.cache"
    if [ ! -f "$archive" ]; then
      url=$(curl -sSL "https://api.github.com/repos/zhongfly/mpv-winbuild/releases/latest" \
        | grep -oE '"browser_download_url": "[^"]*mpv-dev-x86_64-[^"]*\.7z"' \
        | head -1 | sed 's/.*: "//; s/"$//')
      echo "下载 $url"
      curl -sSL --retry 3 -C - -o "$archive" "$url"
    fi
    extractor=""
    for candidate in 7z 7za 7zr "$root/.cache/7zr.exe"; do
      if command -v "$candidate" >/dev/null 2>&1; then extractor="$candidate"; break; fi
      [ -x "$candidate" ] && { extractor="$candidate"; break; }
    done
    if [ -z "$extractor" ]; then
      curl -sSL -o "$root/.cache/7zr.exe" https://www.7-zip.org/a/7zr.exe
      extractor="$root/.cache/7zr.exe"
    fi
    "$extractor" x -y "$archive" -o"$root/.cache/mpv" >/dev/null
    cp "$root/.cache/mpv/libmpv-2.dll" "$dest/$libname"
    ;;
  macos-*)
    if command -v brew >/dev/null 2>&1; then
      brew list mpv >/dev/null 2>&1 || brew install mpv
      cp -L "$(brew --prefix)/lib/libmpv.2.dylib" "$dest/$libname"
      # Homebrew 的 dylib 依赖一堆同目录的库，打包时要一起带走。
      echo "提示：分发 .app 时把 $(brew --prefix)/lib 下 libmpv 依赖的 dylib 一并复制，"
      echo "      并用 install_name_tool -change 修正 rpath。"
    else
      echo "请先安装 Homebrew 或手动把 libmpv.2.dylib 放到 $dest" >&2
      exit 1
    fi
    ;;
  linux-*)
    echo "请用发行版包管理器安装 libmpv：" >&2
    echo "  Debian/Ubuntu: sudo apt install libmpv2" >&2
    echo "  Fedora:        sudo dnf install mpv-libs" >&2
    echo "  Arch:          sudo pacman -S mpv" >&2
    exit 1
    ;;
esac

ls -la "$dest"
