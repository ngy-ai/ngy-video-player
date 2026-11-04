use super::*;
use crate::clip::{ClipRange, CutKind};
use std::path::{Path, PathBuf};

fn request(options: ExportOptions) -> ClipRequest {
    ClipRequest {
        input: PathBuf::from("movie.mkv"),
        output: PathBuf::from(format!("movie - 片段.{}", options.format.extension())),
        range: ClipRange {
            start: 10.0,
            end: 25.0,
        },
        options,
        burn: None,
        loudness: None,
        kind: CutKind::Keep,
        duration: 600.0,
    }
}

fn args(options: ExportOptions) -> Vec<String> {
    command_args(options, None)
}

/// 带上「已经抽好的字幕文件」拼参数，验证烧录那一路。
fn command_args(options: ExportOptions, burn: Option<&Path>) -> Vec<String> {
    let encoder = options.codec.software();
    build_command(Path::new("ffmpeg"), &request(options), encoder, burn)
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

fn has(args: &[String], needle: &[&str]) -> bool {
    args.windows(needle.len()).any(|slice| {
        slice
            .iter()
            .zip(needle)
            .all(|(arg, want)| arg.as_str() == *want)
    })
}

fn count(args: &[String], value: &str) -> usize {
    args.iter().filter(|arg| arg.as_str() == value).count()
}

mod encoding;
mod filters;
mod outputs;
