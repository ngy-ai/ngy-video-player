#!/usr/bin/env bash
#
# 把 release 产物装进 .app。
#
# macOS 的文件关联必须有 bundle：LaunchServices 靠 CFBundleIdentifier 认人，
# 裸二进制没有身份，也就谈不上被设成默认播放器。所以这个脚本是 macOS 上
# 「设置 → 设为默认视频播放器」的前置条件。
#
# 用法：bash scripts/bundle-macos.sh
set -euo pipefail

cd "$(dirname "$0")/.."

APP_NAME="ngy-video-player"
APP="dist/$APP_NAME.app"

cargo build --release --bin "$APP_NAME"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Frameworks"

# 目录名和 CFBundleExecutable 必须一致，程序里按 .app 的包名推算可执行文件名。
cp "target/release/$APP_NAME" "$APP/Contents/MacOS/$APP_NAME"

# libmpv 一并带走：程序会先找可执行文件目录，再找 Contents/Frameworks
# （见 src/player/ffi.rs）。找不到就跳过，别让缺库挡住打包。
shopt -s nullglob
for lib in libs/macos-*/libmpv*.dylib; do
  cp "$lib" "$APP/Contents/Frameworks/"
done
shopt -u nullglob

# 这里只写一份「最小身份」的 Info.plist。完整的 CFBundleDocumentTypes 由程序在
# 注册关联时补全——扩展名清单以 src/formats.rs 为唯一来源，脚本不再抄一遍，
# 抄漏一个扩展名就是一个格式静默失联。
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleIdentifier</key>
	<string>com.ngy.video-player</string>
	<key>CFBundleName</key>
	<string>ngy-video-player</string>
	<key>CFBundleExecutable</key>
	<string>ngy-video-player</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
PLIST

echo "已生成 $APP"
echo "接下来：open \"$APP\"，在 设置 → 设为默认视频播放器… 里完成注册。"
