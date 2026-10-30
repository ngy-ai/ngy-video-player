#!/usr/bin/env bash
# 准备当前平台的 ffmpeg 可执行文件。
#
# 目的：裁切片段需要调用 ffmpeg 重编码。release 构建会把 `libs/<平台>/ffmpeg{,.exe}`
# 内嵌进可执行文件（见 build.rs 与 src/platform/ffmpeg.rs），分发时仍然只有一个 exe，
# 不要求用户自己装 ffmpeg。
#
# 用法：
#   ./scripts/fetch-ffmpeg.sh            # 准备到 libs/<平台>/
#   ./scripts/fetch-ffmpeg.sh --system   # 只检查系统里有没有，不复制
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*)
    platform="windows-x86_64"
    [ "$(uname -m)" = "aarch64" ] && platform="windows-arm64"
    exe="ffmpeg.exe"
    ;;
  Darwin)
    platform="macos-arm64"
    [ "$(uname -m)" = "x86_64" ] && platform="macos-x86_64"
    exe="ffmpeg"
    ;;
  Linux)
    platform="linux-x86_64"
    [ "$(uname -m)" = "aarch64" ] && platform="linux-arm64"
    exe="ffmpeg"
    ;;
  *)
    echo "不支持的平台: $(uname -s)" >&2
    exit 1
    ;;
esac

dest="$root/libs/$platform"
mkdir -p "$dest"

if [ -f "$dest/$exe" ]; then
  echo "已存在: $dest/$exe"
  exit 0
fi

# 开发机 PATH 里有 ffmpeg 就直接用：开发期自己跑得起来就够了，是否静态链接
# 不影响 `cargo run`。只有打包才需要一份能单独带走的可执行文件。
if command -v ffmpeg >/dev/null 2>&1; then
  found=$(command -v ffmpeg)
  if [ "${1:-}" = "--system" ]; then
    echo "系统里已有 ffmpeg: $found"
    exit 0
  fi
  echo "复制 $found → $dest/$exe"
  cp -L "$found" "$dest/$exe"
  exit 0
fi

if [ "${1:-}" = "--system" ]; then
  echo "系统里没有找到 ffmpeg" >&2
  exit 1
fi

case "$platform" in
  windows-*)
    # BtbN 的 win64-gpl 包：单个静态 ffmpeg.exe，不依赖 MinGW 运行库。
    # ARM64 也用这一份：Windows 能模拟运行 x64 程序，且上游没有 arm64 构建。
    archive="$root/.cache/ffmpeg-win64.zip"
    mkdir -p "$root/.cache"
    if [ ! -f "$archive" ]; then
      url=$(curl -sSL "https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/latest" \
        | grep -oE '"browser_download_url": "[^"]*ffmpeg-master-latest-win64-gpl\.zip"' \
        | head -1 | sed 's/.*: "//; s/"$//')
      if [ -z "$url" ]; then
        echo "没找到 ffmpeg 下载地址：GitHub API 可能被限流，稍后重试" >&2
        exit 1
      fi
      echo "下载 $url"
      curl -sSL --retry 3 -C - -o "$archive" "$url"
    fi
    if command -v unzip >/dev/null 2>&1; then
      unzip -o -j "$archive" "*/bin/ffmpeg.exe" -d "$dest" >/dev/null
    else
      rm -rf "$root/.cache/ffmpeg-win64"
      powershell -NoProfile -Command \
        "Expand-Archive -Force '$archive' '$root/.cache/ffmpeg-win64'" >/dev/null
      cp "$root/.cache/ffmpeg-win64/ffmpeg-master-latest-win64-gpl/bin/ffmpeg.exe" "$dest/$exe"
    fi
    ;;
  *)
    echo "请手动准备静态链接的 ffmpeg 并放到 $dest/$exe" >&2
    echo "  macOS: Homebrew 的 ffmpeg 依赖大量 dylib，不能单独内嵌，需要 static 构建" >&2
    echo "  Linux: 用 johnvansickle 的 static 构建，或发行版包管理器安装后再内嵌" >&2
    exit 1
    ;;
esac

ls -la "$dest"
