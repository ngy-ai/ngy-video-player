//! 逐字 / 卡拉 OK 时间轴：把一条字幕的 `[start, end)` 按每个字的权重切开。
//!
//! 权重是**相对值**，不是绝对时长：改字幕起止点时不用重排逐字节拍，导出 / 预览都按
//! 当前区间等比例换算。这也让工程文件里存的是一小串整数，而不是几十个时间点。
//!
//! 换行只是排版，不占节拍（ASS 里写成 `\N`）；所以权重的个数等于「看得见的字」数。
//! `\k` 标签里的数字是厘秒（ASS 的时间粒度），分配时按最大余数法凑够 Dialogue 的
//! 整段时长，否则 libass 会把最后一段拖长，节拍就歪了。

use crate::timeline::Tick;

/// 打开逐字动画时每个字的默认权重（都一样 = 匀速）。
pub const DEFAULT_WEIGHT: u32 = 100;

/// 有权重的是看得见的字：换行与回车不占节拍。
pub fn visible_count(text: &str) -> usize {
    text.chars().filter(|ch| *ch != '\n' && *ch != '\r').count()
}

/// 文字改过之后把权重补齐 / 裁掉，并保证都是正整数。
///
/// 空数组是「没开逐字」的标记，所以这里不会因为字数变了就凭空打开动画。
/// 新补的字沿用最后一个字的权重：用户拉长句子时，新字跟着上一字的节奏走。
pub fn fit(weights: &[u32], text: &str) -> Vec<u32> {
    if weights.is_empty() {
        return Vec::new();
    }
    let count = visible_count(text);
    let mut fitted: Vec<u32> = weights
        .iter()
        .take(count)
        .map(|weight| (*weight).max(1))
        .collect();
    let tail = fitted.last().copied().unwrap_or(DEFAULT_WEIGHT);
    fitted.resize(count, tail);
    fitted
}

/// 每个字的**结束偏移**（相对字幕起点）。最后一段钉在总时长上，整数除法的
/// 余数不会让最后一个字早一帧收尾。
pub fn offsets(weights: &[u32], duration: Tick) -> Vec<Tick> {
    if weights.is_empty() {
        return Vec::new();
    }
    let total: u128 = weights.iter().map(|weight| u128::from(*weight)).sum();
    let span = u128::try_from(duration.micros().max(0)).unwrap_or(0);
    let last = weights.len() - 1;
    let mut elapsed: u128 = 0;
    let mut out = Vec::with_capacity(weights.len());
    for (index, weight) in weights.iter().enumerate() {
        if index == last {
            elapsed = span;
        } else {
            elapsed += span * u128::from(*weight) / total.max(1);
        }
        out.push(Tick::from_micros(elapsed.min(i64::MAX as u128) as i64));
    }
    out
}

/// 播放头落在第几个字上。`None` 表示还没开始或已经唱完。
pub fn index_at(offsets: &[Tick], elapsed: Tick) -> Option<usize> {
    let end = *offsets.last()?;
    if elapsed < Tick::ZERO || elapsed >= end {
        return None;
    }
    offsets.iter().position(|boundary| elapsed < *boundary)
}

/// 预览用：按「已经唱过几个字」把文字切成两段，换行 / 反斜杠压成空格
/// （与 `SubtitleItem::label()` 的口径一致），两段拼起来仍是原来那一行。
pub fn split(text: &str, sung: usize) -> (String, String) {
    let mut done = String::new();
    let mut rest = String::new();
    let mut index = 0usize;
    for ch in text.chars() {
        let target = if index < sung { &mut done } else { &mut rest };
        if ch == '\n' || ch == '\r' || ch == '\\' {
            target.push(' ');
        } else {
            target.push(ch);
        }
        if ch != '\n' && ch != '\r' {
            index += 1;
        }
    }
    (done.trim_start().to_string(), rest.trim_end().to_string())
}

/// 面板上拖分界线：把第 `index` 与后一个字的权重和按 `fraction`（0..1，在这两个字
/// 组成的小段里的位置）重新切开，其余字与整条字幕的时长都不动。
///
/// 返回的 `(左, 右)` 都是正数且和不变；下标越界返回 `None`。
pub fn move_boundary(weights: &[u32], index: usize, fraction: f32) -> Option<(u32, u32)> {
    if index + 1 >= weights.len() {
        return None;
    }
    let pair = weights[index].saturating_add(weights[index + 1]);
    let fraction = if fraction.is_finite() {
        fraction.clamp(0.0, 1.0)
    } else {
        0.5
    };
    let left = (fraction * pair as f32)
        .round()
        .clamp(1.0, pair.saturating_sub(1).max(1) as f32) as u32;
    Some((left, pair.saturating_sub(left).max(1)))
}

/// 把 Dialogue 的厘秒时长按权重分给每个字，返回的总和**严格等于** `total`。
///
/// 先向下取整，再把余下的厘秒按余数从大到小补给对应字（下标小的优先，保证稳定）。
pub fn centis(weights: &[u32], total: i64) -> Vec<i64> {
    let total = total.max(0);
    if weights.is_empty() {
        return Vec::new();
    }
    if total == 0 {
        return vec![0; weights.len()];
    }
    let sum: u128 = weights
        .iter()
        .map(|weight| u128::from(*weight))
        .sum::<u128>()
        .max(1);
    let mut out = Vec::with_capacity(weights.len());
    let mut remainders: Vec<(usize, u128)> = Vec::with_capacity(weights.len());
    let mut assigned = 0i64;
    let total_u = u128::try_from(total).unwrap_or(0);
    for (index, weight) in weights.iter().enumerate() {
        let numerator = total_u * u128::from(*weight);
        let value = (numerator / sum) as i64;
        out.push(value);
        assigned += value;
        remainders.push((index, numerator % sum));
    }
    remainders.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (index, _) in remainders.into_iter().take((total - assigned) as usize) {
        out[index] += 1;
    }
    out
}

#[cfg(test)]
mod tests;
