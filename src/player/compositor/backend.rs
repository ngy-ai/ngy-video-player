//! 合成后端的选择（M10-T02 的接口部分）。
//!
//! ADR 0008 定的路线是 **A（多实例 + CPU 合成）→ B（多实例 + GPU 合成）**，并且 A 永远
//! 保留为回退。这里先把「谁来叠这一帧」抽成一个可切换的点：
//!
//! * [`Backend::Cpu`]——现在的实现，`FrameBuffer::blend_into` 逐像素做 alpha。
//! * [`Backend::Gpu`]——目标实现（Windows 上 D3D11 纹理 → wgpu compute）。**尚未实现**：
//!   它要的是能跑的显卡 + 能与 mpv 的渲染上下文互操作的设备，改完必须在真机上逐帧比对
//!   才敢说「和导出一致」，没有验证就不该把这条路打开。
//!
//! 所以 [`detect`] 现在只会返回 CPU，并在用户显式要求 GPU 时给出一条能看懂的降级说明，
//! 而不是静默换成 CPU。等 B 落地时，只有这个文件里「返回 Gpu」那几行要改。

/// 用谁来把多层叠成一帧。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Backend {
    /// 逐像素 CPU 混合。
    Cpu,
    /// GPU 合成（尚未实现）。
    Gpu,
}

impl Backend {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Backend::Cpu => "CPU 合成",
            Backend::Gpu => "GPU 合成",
        }
    }
}

/// 环境变量：`NGY_COMPOSITE_BACKEND=cpu|gpu`，给排障与将来的开关留的入口。
pub(crate) const ENV: &str = "NGY_COMPOSITE_BACKEND";

/// 选后端。返回值第二项是「为什么不是想要的那个」，给日志与界面提示用。
///
/// 只认两个值：`cpu`（默认）与 `gpu`。写 `gpu` 而现在没有 GPU 实现时**退回 CPU 并说明**，
/// 静默降级会让人以为测的是 GPU 路径。
pub(crate) fn detect(requested: Option<&str>, gpu_supported: bool) -> (Backend, Option<String>) {
    match requested
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("gpu") if gpu_supported => (Backend::Gpu, None),
        Some("gpu") => (
            Backend::Cpu,
            Some(format!(
                "GPU 合成后端还没实现（要真机验证才敢开），这一轮仍用 CPU 合成；\
                 想确认用的是 CPU 可以设 {ENV}=cpu"
            )),
        ),
        Some("cpu") | None | Some("") | Some(_) => (Backend::Cpu, None),
    }
}

/// 从环境变量里读一次并选后端。合成线程启动时调用一次。
pub(crate) fn detect_from_env(gpu_supported: bool) -> (Backend, Option<String>) {
    let requested = std::env::var(ENV).ok();
    detect(requested.as_deref(), gpu_supported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 默认走_cpu() {
        assert_eq!(detect(None, false), (Backend::Cpu, None));
        assert_eq!(detect(None, true), (Backend::Cpu, None));
        assert_eq!(detect(Some("cpu"), true), (Backend::Cpu, None));
        // 乱写的值也走 CPU，不报错：环境变量是给人手敲的。
        assert_eq!(detect(Some("vulkan"), false), (Backend::Cpu, None));
    }

    #[test]
    fn 要_gpu_但没实现时退回并说明原因() {
        let (backend, reason) = detect(Some("gpu"), false);
        assert_eq!(backend, Backend::Cpu);
        let reason = reason.expect("必须给出降级原因");
        assert!(reason.contains("GPU"), "{reason}");
        assert!(reason.contains("CPU"), "{reason}");
    }

    #[test]
    fn 大写与空格都认() {
        assert_eq!(detect(Some(" GPU "), false).0, Backend::Cpu);
        // 真实现之后这一条会变成 `Backend::Gpu`，届时这里要一起改。
        assert!(detect(Some(" GPU "), false).1.is_some());
    }
}
