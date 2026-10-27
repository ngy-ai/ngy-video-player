//! Linux 的纯逻辑：扩展名 → MIME 映射、`.desktop` 生成、`mimeapps.list` 维护。
//!
//! 这里只处理字符串和文件内容，不碰任何系统路径，因此在任何平台上都能跑单元测试——
//! 注册代码只能在 Linux 上执行，但它的逻辑没理由只能在 Linux 上验证。
//!
//! 之所以自己写 `mimeapps.list` 而不是调 `xdg-mime`：那个工具不一定装了，而它干的事
//! 就是把一行 `mime= xxx.desktop` 写进这个文件。自己写还省了一次进程启动。

use std::collections::{BTreeMap, HashMap};

/// 系统没有 `shared-mime-info` 时的兜底映射。
///
/// 只列最常见的几种：宁可少注册一个格式，也不要把 MIME 猜错——填错的 MIME 会让
/// `xdg-open` 把文件交给不认识它的程序。
const BUILTIN: &[(&str, &str)] = &[
    ("mp4", "video/mp4"),
    ("m4v", "video/x-m4v"),
    ("mkv", "video/x-matroska"),
    ("webm", "video/webm"),
    ("mov", "video/quicktime"),
    ("avi", "video/x-msvideo"),
    ("flv", "video/x-flv"),
    ("wmv", "video/x-ms-wmv"),
    ("asf", "video/x-ms-asf"),
    ("ts", "video/mp2t"),
    ("m2ts", "video/mp2t"),
    ("mpg", "video/mpeg"),
    ("mpeg", "video/mpeg"),
    ("ogv", "video/ogg"),
    ("ogg", "video/ogg"),
    ("3gp", "video/3gpp"),
    ("dv", "video/x-dv"),
    ("mxf", "application/mxf"),
    ("m3u", "audio/x-mpegurl"),
    ("m3u8", "application/vnd.apple.mpegurl"),
    ("pls", "audio/x-scpls"),
    ("xspf", "application/xspf+xml"),
    ("cue", "application/x-cue"),
];

/// 解析 `shared-mime-info` 的 `globs2`（也兼容老的 `globs`）。
///
/// `globs2` 每行是 `权重:MIME:模式`，老的 `globs` 只有 `MIME:模式`。同一扩展名被多个
/// MIME 声明时取权重最高的那个。只收 `*.ext` 形式的单后缀规则：`*.tar.gz` 这类复合
/// 模式按最后一段归属会张冠李戴。
pub fn parse_globs(text: &str) -> HashMap<String, (i32, String)> {
    let mut table: HashMap<String, (i32, String)> = HashMap::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.splitn(3, ':').collect();
        let (weight, mime, glob) = match parts.as_slice() {
            [weight, mime, glob] => match weight.trim_start_matches('!').parse::<i32>() {
                Ok(weight) => (weight, *mime, *glob),
                // 老格式第一段是 MIME 而不是权重。
                Err(_) => (50, *weight, *mime),
            },
            [mime, glob] => (50, *mime, *glob),
            _ => continue,
        };

        // `*.mp4:cs` 这种带 flag 的行，冒号后面的部分不属于模式。
        let glob = glob.split(':').next().unwrap_or_default();
        let Some(extension) = glob.strip_prefix("*.") else {
            continue;
        };
        if extension.is_empty() || extension.contains('.') || extension.contains('*') {
            continue;
        }
        let extension = extension.to_ascii_lowercase();
        match table.get(&extension) {
            Some((best, _)) if *best >= weight => {}
            _ => {
                table.insert(extension, (weight, mime.to_string()));
            }
        }
    }

    table
}

/// 把「我们要注册的扩展名」翻译成 MIME，系统数据库优先、内置表兜底。
pub fn resolve(globs: Option<&str>, extensions: &[&str]) -> BTreeMap<String, String> {
    let system = globs.map(parse_globs).unwrap_or_default();
    let builtin: HashMap<&str, &str> = BUILTIN.iter().copied().collect();

    let mut mapping = BTreeMap::new();
    for extension in extensions {
        let key = extension.to_ascii_lowercase();
        if let Some((_, mime)) = system.get(&key) {
            mapping.insert(key, mime.clone());
        } else if let Some(mime) = builtin.get(key.as_str()) {
            mapping.insert(key, (*mime).to_string());
        }
    }
    mapping
}

/// `.desktop` 里 `MimeType=` 用的去重列表。
pub fn unique_mimes(mapping: &BTreeMap<String, String>) -> Vec<String> {
    let mut mimes: Vec<String> = mapping.values().cloned().collect();
    mimes.sort();
    mimes.dedup();
    mimes
}

/// `Exec` 里的路径要按 desktop entry 规则转义：引号、反引号、`$`、`\` 前加反斜杠，
/// `%` 要写成 `%%`，否则会被当成字段代码。
fn quote_exec(path: &str) -> String {
    let mut quoted = String::from("\"");
    for character in path.chars() {
        match character {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(character);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(character),
        }
    }
    quoted.push('"');
    quoted
}

/// 生成启动器文件。`%f` 表示「把双击的那个文件传进来」。
pub fn desktop_entry(exe: &str, icon: &str, mimes: &[String]) -> String {
    let mut entry = String::new();
    entry.push_str("[Desktop Entry]\n");
    entry.push_str("Type=Application\n");
    entry.push_str("Version=1.0\n");
    entry.push_str("Name=ngy-video-player\n");
    entry.push_str("GenericName=Video Player\n");
    entry.push_str("Comment=轻量视频播放器（GPUI + libmpv）\n");
    entry.push_str(&format!("Exec={} %f\n", quote_exec(exe)));
    entry.push_str(&format!("Icon={icon}\n"));
    entry.push_str("Terminal=false\n");
    entry.push_str("StartupNotify=true\n");
    entry.push_str("Categories=AudioVideo;Video;Player;\n");
    entry.push_str(&format!("MimeType={};\n", mimes.join(";")));
    entry
}

/// 从启动器里读回 Exec 的第一个参数和 MimeType 列表。
pub fn parse_desktop(text: &str) -> (Option<String>, Vec<String>) {
    let mut executable = None;
    let mut mimes = Vec::new();
    let mut in_entry = false;

    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        if let Some(value) = line.strip_prefix("Exec=") {
            executable = first_exec_argument(value);
        } else if let Some(value) = line.strip_prefix("MimeType=") {
            mimes = value
                .split(';')
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect();
        }
    }

    (executable, mimes)
}

fn first_exec_argument(command: &str) -> Option<String> {
    let rest = command.strip_prefix('"')?;
    let mut argument = String::new();
    let mut characters = rest.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' => argument.extend(characters.next()),
            '"' => return Some(argument),
            _ => argument.push(character),
        }
    }
    None
}

#[derive(Debug, Default)]
struct Ini {
    preamble: Vec<String>,
    sections: Vec<(String, Vec<String>)>,
}

impl Ini {
    fn parse(text: &str) -> Self {
        let mut ini = Self::default();
        let mut current: Option<usize> = None;
        for line in text.lines() {
            let trimmed = line.trim();
            let header = trimmed
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'));
            if let Some(name) = header {
                ini.sections.push((name.to_string(), Vec::new()));
                current = Some(ini.sections.len() - 1);
            } else if let Some(index) = current {
                ini.sections[index].1.push(line.to_string());
            } else {
                ini.preamble.push(line.to_string());
            }
        }
        ini
    }

    fn render(&self) -> String {
        let mut text = String::new();
        for line in &self.preamble {
            text.push_str(line);
            text.push('\n');
        }
        for (name, lines) in &self.sections {
            text.push('[');
            text.push_str(name);
            text.push_str("]\n");
            for line in lines {
                text.push_str(line);
                text.push('\n');
            }
        }
        text
    }

    fn section_mut(&mut self, name: &str) -> &mut Vec<String> {
        match self.sections.iter().position(|(header, _)| header == name) {
            Some(index) => &mut self.sections[index].1,
            None => {
                self.sections.push((name.to_string(), Vec::new()));
                let index = self.sections.len() - 1;
                &mut self.sections[index].1
            }
        }
    }
}

/// `mimeapps.list` 里的值可能带结尾分号（`[Added Associations]` 允许一个 MIME 列多个
/// 启动器），比较前先去掉。
fn is_ours(value: &str, desktop_id: &str) -> bool {
    value.trim().trim_end_matches(';') == desktop_id
}

/// 把本程序写成这些 MIME 的默认处理程序，其余内容原样保留。
pub fn set_defaults(original: &str, desktop_id: &str, mimes: &[String]) -> String {
    let mut ini = Ini::parse(original);
    let lines = ini.section_mut("Default Applications");

    // 先清掉自己以前写的行：一是防止格式清单变了以后留下孤儿条目，
    // 二是保证同一个 MIME 不会出现两行（后一行会覆盖前一行，行为不直观）。
    lines.retain(|line| match line.split_once('=') {
        Some((key, value)) => {
            !is_ours(value, desktop_id) && !mimes.iter().any(|mime| mime == key.trim())
        }
        None => true,
    });

    for mime in mimes {
        lines.push(format!("{mime}={desktop_id}"));
    }
    ini.render()
}

/// 撤销本程序写下的条目，别的应用的选择保持不动。
pub fn remove_defaults(original: &str, desktop_id: &str) -> String {
    let mut ini = Ini::parse(original);
    for (name, lines) in &mut ini.sections {
        if name == "Default Applications" || name == "Added Associations" {
            lines.retain(|line| match line.split_once('=') {
                Some((_, value)) => !is_ours(value, desktop_id),
                None => true,
            });
        }
    }
    ini.render()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析_globs2_取权重最高的_mime() {
        let text = "\
# 注释
50:video/mp4:*.mp4
80:audio/mp4:*.mp4
50:application/gzip:*.tar.gz
50:video/x-matroska:*.mkv:cs
";
        let table = parse_globs(text);
        assert_eq!(table["mp4"].1, "audio/mp4", "权重高的应当胜出");
        assert_eq!(table["mkv"].1, "video/x-matroska");
        assert!(!table.contains_key("gz"), "复合模式不该按最后一段收下");
    }

    #[test]
    fn 解析老式_globs_格式() {
        let table = parse_globs("video/mp4:*.mp4\n");
        assert_eq!(table["mp4"].1, "video/mp4");
    }

    #[test]
    fn 系统没有数据库时落回内置表() {
        let mapping = resolve(None, &["mp4", "mkv", "jpg"]);
        assert_eq!(mapping["mp4"], "video/mp4");
        assert_eq!(mapping["mkv"], "video/x-matroska");
        assert!(!mapping.contains_key("jpg"), "内置表不管图片");
    }

    #[test]
    fn 系统数据库优先于内置表() {
        let mapping = resolve(Some("80:video/custom:*.mp4\n"), &["mp4"]);
        assert_eq!(mapping["mp4"], "video/custom");
    }

    #[test]
    fn 多个扩展名共用一个_mime_时只写一次() {
        // ts 和 m2ts 都是 video/mp2t，MimeType 里写两遍没有意义。
        let mapping = resolve(None, &["ts", "m2ts", "mp4"]);
        let mimes = unique_mimes(&mapping);
        assert_eq!(mimes.iter().filter(|mime| *mime == "video/mp2t").count(), 1);
        assert!(mimes.contains(&"video/mp4".to_string()));
    }

    #[test]
    fn 启动器里的_exec_能读回来() {
        let entry = desktop_entry(
            r#"/opt/my player/ngy"#,
            "/home/u/.local/share/icons/hicolor/256x256/apps/ngy-video-player.png",
            &["video/mp4".to_string()],
        );
        assert!(entry.contains(r#"Exec="/opt/my player/ngy" %f"#));

        let (executable, mimes) = parse_desktop(&entry);
        assert_eq!(executable.as_deref(), Some("/opt/my player/ngy"));
        assert_eq!(mimes, vec!["video/mp4".to_string()]);
    }

    #[test]
    fn 默认应用表只替换自己的行() {
        let original = "\
[Added Associations]
video/mp4=vlc.desktop;

[Default Applications]
video/mp4=vlc.desktop
text/plain=gedit.desktop
";
        let updated = set_defaults(
            original,
            "ngy-video-player.desktop",
            &["video/mp4".to_string()],
        );

        assert!(
            updated.contains("text/plain=gedit.desktop"),
            "别人的条目要留着"
        );
        assert!(updated.contains("video/mp4=ngy-video-player.desktop"));
        // `[Added Associations]` 里那一行是别的语义，允许留着；只要求默认应用表干净。
        assert!(
            !updated.contains("video/mp4=vlc.desktop\n"),
            "默认应用表里同一个 MIME 只留一行"
        );
        assert!(updated.contains("[Added Associations]"), "其它小节原样保留");

        // 重复执行结果不变，这才是「幂等」。
        assert_eq!(
            updated,
            set_defaults(
                &updated,
                "ngy-video-player.desktop",
                &["video/mp4".to_string()]
            )
        );
    }

    #[test]
    fn 缺少小节时补一个() {
        let updated = set_defaults("", "ngy-video-player.desktop", &["video/mp4".to_string()]);
        assert_eq!(
            updated,
            "[Default Applications]\nvideo/mp4=ngy-video-player.desktop\n"
        );
    }

    #[test]
    fn 撤销只删自己的条目() {
        let original = "\
[Default Applications]
video/mp4=ngy-video-player.desktop
video/webm=firefox.desktop
[Added Associations]
video/mkv=ngy-video-player.desktop;
";
        let cleaned = remove_defaults(original, "ngy-video-player.desktop");
        assert!(!cleaned.contains("ngy-video-player.desktop"));
        assert!(cleaned.contains("video/webm=firefox.desktop"));
    }
}
