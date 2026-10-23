// 发布版不弹控制台黑框；debug 版保留，方便直接看日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ngy_video_player::app::{self, LaunchOptions};
use std::path::PathBuf;

fn main() {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut fullscreen = false;

    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--fullscreen" | "-f" => fullscreen = true,
            "--help" | "-h" => {
                print_help();
                return;
            }
            "--version" | "-V" => {
                println!("ngy-video-player {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            other if other.starts_with('-') => {
                eprintln!("忽略未知参数: {other}");
            }
            other => {
                let path = PathBuf::from(other);
                if path.exists() {
                    paths.push(path);
                } else {
                    eprintln!("路径不存在，已跳过: {}", path.display());
                }
            }
        }
    }

    if let Err(error) = app::run(LaunchOptions { paths, fullscreen }) {
        eprintln!("启动失败: {error:#}");
        ngy_video_player::log_error!("启动失败: {error:#}");
    }
}

fn print_help() {
    println!(
        "ngy-video-player {}\n\
         \n\
         用法: ngy-video-player [选项] [文件或目录...]\n\
         \n\
         选项:\n\
          -f, --fullscreen   以全屏启动\n\
          -h, --help         显示本帮助\n\
          -V, --version      显示版本\n\
         \n\
         环境变量:\n\
          NGY_MPV_LIB        指定 libmpv 库文件路径\n\
          NGY_MPV_VERBOSE=1  输出 libmpv 详细日志（排查解码问题）\n\
          NGY_VIDEO_LOG_DIR  自定义日志目录\n\
          NGY_VIDEO_CONFIG_DIR 自定义配置目录",
        env!("CARGO_PKG_VERSION")
    );
}
