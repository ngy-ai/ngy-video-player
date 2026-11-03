//! EBU R128 两遍响度标准化的第一遍：读数与解析。
//!
//! 单遍 `loudnorm` 一边统计一边调增益，遇到忽大忽小的片源会「先压后放」，听感
//! 不自然。两遍做法是先空跑一遍把整段统计出来，第二遍按测量值做一次线性增益，
//! 动态不被压扁。
//!
//! ffmpeg 没有把测量值输出到文件的能力，只能让它打到 stderr 上再抓回来；格式是
//! 一段 JSON，这里手写解析而不是引入 serde——总共五个字段，为它加一个依赖不值得。

/// 目标响度参数。单遍与两遍必须是同一串，否则测量值对不上。
pub const TARGETS: &str = "I=-16:TP=-1.5:LRA=11";

/// 第一遍测出来的五项数值，第二遍原样喂回去。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Loudness {
    pub input_i: f64,
    pub input_tp: f64,
    pub input_lra: f64,
    pub input_thresh: f64,
    pub target_offset: f64,
}

impl Loudness {
    /// 第二遍用的滤镜参数：带上测量值，并开启线性模式。
    pub fn filter(&self) -> String {
        format!(
            "loudnorm={TARGETS}:measured_I={:.2}:measured_TP={:.2}:measured_LRA={:.2}:\
             measured_thresh={:.2}:offset={:.2}:linear=true",
            self.input_i, self.input_tp, self.input_lra, self.input_thresh, self.target_offset
        )
    }
}

/// 从 ffmpeg 的 stderr 里抠出最后一段 `{ ... }`，读五个字段。
///
/// 取最后一段是因为同一个进程可能打印过别的 JSON（`-progress` 之类）；loudnorm 的
/// 统计一定出现在收尾处。
pub(crate) fn parse(text: &str) -> Option<Loudness> {
    let start = text.rfind('{')?;
    let end = text[start..].find('}')? + start;
    let block = &text[start..=end];
    Some(Loudness {
        input_i: field(block, "input_i")?,
        input_tp: field(block, "input_tp")?,
        input_lra: field(block, "input_lra")?,
        input_thresh: field(block, "input_thresh")?,
        target_offset: field(block, "target_offset")?,
    })
}

/// 读一个字段。ffmpeg 把数值写成字符串（`"input_i" : "-23.06"`），
/// 所以先找到冒号，再跳掉引号取数字。
fn field(block: &str, name: &str) -> Option<f64> {
    let key = format!("\"{name}\"");
    let after_key = &block[block.find(&key)? + key.len()..];
    let after_colon = &after_key[after_key.find(':')? + 1..];
    let begin = after_colon.find(|ch: char| ch == '-' || ch.is_ascii_digit())?;
    let rest = &after_colon[begin..];
    let end = rest
        .find(|ch: char| !(ch == '-' || ch == '.' || ch.is_ascii_digit()))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ffmpeg 真实输出的一段（节选）。
    const SAMPLE: &str = "\
[Parsed_loudnorm_0 @ 000001] \n\
{\n\
\t\"input_i\" : \"-23.06\",\n\
\t\"input_tp\" : \"-6.02\",\n\
\t\"input_lra\" : \"7.20\",\n\
\t\"input_thresh\" : \"-33.10\",\n\
\t\"output_i\" : \"-16.01\",\n\
\t\"output_tp\" : \"-1.51\",\n\
\t\"target_offset\" : \"0.01\"\n\
}";

    #[test]
    fn 从真实输出里读出五项测量值() {
        let loudness = parse(SAMPLE).expect("应当能解析");
        assert_eq!(loudness.input_i, -23.06);
        assert_eq!(loudness.input_tp, -6.02);
        assert_eq!(loudness.input_lra, 7.2);
        assert_eq!(loudness.input_thresh, -33.1);
        assert_eq!(loudness.target_offset, 0.01);
    }

    #[test]
    fn 没有测量值时不猜() {
        assert!(parse("").is_none());
        assert!(parse("{\n \"input_i\" : \"-inf\"\n}").is_none());
        // 有 JSON 但缺字段
        assert!(parse("{\n \"input_i\" : \"-23.06\"\n}").is_none());
    }

    #[test]
    fn 第二遍把测量值写进滤镜() {
        let loudness = parse(SAMPLE).unwrap();
        let filter = loudness.filter();
        assert!(filter.starts_with("loudnorm=I=-16:TP=-1.5:LRA=11:measured_I=-23.06"));
        assert!(filter.contains("measured_TP=-6.02"));
        assert!(filter.contains("offset=0.01"));
        assert!(filter.ends_with("linear=true"));
    }
}
