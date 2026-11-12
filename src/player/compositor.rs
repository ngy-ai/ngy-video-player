//! 多轨软件合成预览（M10-T01）：每条视频轨一个 mpv，SW 渲染到内存后由 CPU 叠起来。
//!
//! 为什么要在自己这边合成：预览的主实例一次只解一路画面（`player::engine`），
//! 多轨叠加、画中画这类「改一下要立刻看到」的东西它给不了。ADR 0008 定的路线是
//! 「多实例 + 合成」，第一步先做 CPU 版（架构 A），GPU 版（`M10-T02`）之后再来。
//!
//! 三段约定：
//!
//! * **每一路都按同一个画布尺寸渲染**。缩放与黑边交给 mpv（它自己按
//!   `keepaspect` 排），于是合成只剩逐像素的 alpha 混合，不用二次缩放——
//!   1080p 下每帧省掉一次全画面的重采样。
//! * **合成结果仍然走 `PlayerEvent`**。主线程拿到的还是一张 `RenderImage`，
//!   `incoming` / `swap_frame` / 画面排版一行都不用改。
//! * **渲染上下文只在创建它的线程上用**。句柄、上下文、像素缓冲全部留在
//!   `mpv-composite` 线程，界面侧只投递请求。
//!
//! 图层顺序自下而上（`LayerSpec` 列表第 0 个是底），与 `timeline::preview::layers`
//! 的输出一致。一层都没就绪时什么都不发——界面留着上一帧，比闪一下黑好。
//!
//! 文件分工：`handle` 是界面侧句柄与请求枚举，`layer` 是一路 mpv 的加载 / seek /
//! 渲染，`worker` 是合成线程本体（收请求、叠帧、回传）。

mod handle;
mod layer;
mod worker;

use std::time::Duration;

pub(crate) use handle::Compositor;
pub(crate) use layer::LayerSpec;

/// 同时参与合成的视频轨上限。
///
/// ADR 0008 给的是 4 路，第一步只做两路：CPU 合成 1080p 两路已经是 24fps 的边，
/// 四路得等 GPU 后端（`M10-T02`）。超出的轨不参与合成，由界面提示一句。
pub(crate) const MAX_LAYERS: usize = 2;

/// 打开文件的等待上限（与缩略图实例同一档）。
const LOAD_TIMEOUT: Duration = Duration::from_secs(4);
/// 单次 seek 的等待上限。
const SEEK_TIMEOUT: Duration = Duration::from_secs(3);
/// 位置差多少才值得补一次 seek。
const SEEK_TOLERANCE: f64 = 0.08;
/// 空闲时醒来的间隔：既要不影响播放的跟手程度，又不至于空转烧 CPU。
const TICK: Duration = Duration::from_millis(4);
/// 等合成线程回传一帧的上限。它空闲时每 `TICK` 醒一次，正常都在一毫秒内回来。
const SNAPSHOT_TIMEOUT: Duration = Duration::from_millis(400);

#[cfg(test)]
mod tests {
    use crate::player::compositor::layer::normalize_rate;

    #[test]
    fn 播放速率坏值退回原速() {
        assert_eq!(normalize_rate(1.0), 1.0);
        assert_eq!(normalize_rate(2.0), 2.0);
        assert_eq!(normalize_rate(4.0), 4.0);
        // 时间线只会给 1 / 2 / 4；太快的一律夹住，零与负数（倒放 / 定格，本来
        // 也不该送到这里）退回原速，免得 mpv 收到离谱的 speed。
        assert_eq!(normalize_rate(0.0), 1.0);
        assert_eq!(normalize_rate(-2.0), 1.0);
        assert_eq!(normalize_rate(8.0), 4.0);
        assert_eq!(normalize_rate(f64::NAN), 1.0);
        assert_eq!(normalize_rate(f64::INFINITY), 1.0);
    }
}
