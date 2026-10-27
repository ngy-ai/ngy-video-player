//! macOS 的 `Info.plist` 生成与解析。
//!
//! 关联靠 bundle 里的 `CFBundleDocumentTypes` 声明，而扩展名清单以 `formats.rs`
//! 为准。所以 plist 由程序自己生成，而不是让打包脚本再抄一份六十个扩展名——
//! 抄错一个，那个格式就静默地不会被关联到。

const HEADER: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
"#;

const FOOTER: &str = "</dict>\n</plist>\n";

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn push_string(plist: &mut String, key: &str, value: &str) {
    plist.push_str(&format!(
        "\t<key>{}</key>\n\t<string>{}</string>\n",
        escape(key),
        escape(value)
    ));
}

/// 生成完整的 `Info.plist`。`extensions` 为空时只保留 bundle 身份，
/// 不再声明任何文档类型——撤销时就是这么用的。
pub fn info_plist(
    bundle_id: &str,
    executable: &str,
    name: &str,
    version: &str,
    extensions: &[&str],
) -> String {
    let mut plist = String::from(HEADER);

    push_string(&mut plist, "CFBundleIdentifier", bundle_id);
    push_string(&mut plist, "CFBundleName", name);
    push_string(&mut plist, "CFBundleDisplayName", name);
    push_string(&mut plist, "CFBundleExecutable", executable);
    push_string(&mut plist, "CFBundlePackageType", "APPL");
    push_string(&mut plist, "CFBundleShortVersionString", version);
    push_string(&mut plist, "CFBundleVersion", version);
    // UTType* / LS* 这套接口在 macOS 11 之后才齐全，12 起稳定。
    push_string(&mut plist, "LSMinimumSystemVersion", "12.0");
    plist.push_str("\t<key>NSHighResolutionCapable</key>\n\t<true/>\n");

    if !extensions.is_empty() {
        plist.push_str("\t<key>CFBundleDocumentTypes</key>\n\t<array>\n\t\t<dict>\n");
        push_string(&mut plist, "CFBundleTypeName", "Video");
        push_string(&mut plist, "CFBundleTypeRole", "Viewer");
        // Alternate 而不是 Owner：装上来只进候选，不抢用户已经选定的默认播放器。
        push_string(&mut plist, "LSHandlerRank", "Alternate");
        plist.push_str("\t\t\t<key>CFBundleTypeExtensions</key>\n\t\t\t<array>\n");
        for extension in extensions {
            plist.push_str(&format!("\t\t\t\t<string>{}</string>\n", escape(extension)));
        }
        plist.push_str("\t\t\t</array>\n\t\t</dict>\n\t</array>\n");
    }

    plist.push_str(FOOTER);
    plist
}

/// 从 plist 里读回我们声明过的扩展名。只认得自己生成的那一种写法，够用。
pub fn declared_extensions(plist: &str) -> Vec<String> {
    let Some(start) = plist.find("<key>CFBundleTypeExtensions</key>") else {
        return Vec::new();
    };
    let rest = &plist[start..];
    let Some(array_start) = rest.find("<array>") else {
        return Vec::new();
    };
    let rest = &rest[array_start..];
    let Some(array_end) = rest.find("</array>") else {
        return Vec::new();
    };

    let mut extensions = Vec::new();
    let mut cursor = 0;
    let block = &rest[..array_end];
    while let Some(open) = block[cursor..].find("<string>") {
        let value_start = cursor + open + "<string>".len();
        let Some(close) = block[value_start..].find("</string>") else {
            break;
        };
        extensions.push(block[value_start..value_start + close].trim().to_string());
        cursor = value_start + close + "</string>".len();
    }
    extensions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 生成的清单声明了每一个扩展名() {
        let plist = info_plist(
            "com.ngy.video-player",
            "ngy-video-player",
            "ngy-video-player",
            "0.1.0",
            &["mp4", "mkv", "m3u"],
        );
        assert!(plist.contains("<string>com.ngy.video-player</string>"));
        assert!(plist.contains("<key>LSHandlerRank</key>"));
        assert_eq!(
            declared_extensions(&plist),
            vec!["mp4".to_string(), "mkv".to_string(), "m3u".to_string()]
        );
    }

    #[test]
    fn 空清单不再声明文档类型() {
        let plist = info_plist("com.ngy.video-player", "x", "x", "0.1.0", &[]);
        assert!(!plist.contains("CFBundleDocumentTypes"));
        assert!(declared_extensions(&plist).is_empty());
        // 身份信息得留着，否则这个 bundle 就成了一副空壳。
        assert!(plist.contains("CFBundleIdentifier"));
    }

    #[test]
    fn 特殊字符被转义() {
        let plist = info_plist("a&b", "x", "x", "0.1.0", &[]);
        assert!(plist.contains("<string>a&amp;b</string>"));
    }
}
