//! 调色（M9-T11）在导出链里的位置：构图之后、淡入淡出之前、效果之前。
//!
//! 参数映射本身在 `timeline/color.rs`；这里只看它被拼进了哪一段。

use super::*;

/// 调色排在关键帧之后、效果之前：构图与位移定下来再统一色彩，水印不被一起染。
#[test]
fn 调色排在关键帧之后效果之前() {
    let scratch = Scratch::new("color");
    let (mut project, ids) = project_with(&scratch, &[(s(0), s(3), Tick::ZERO)]);
    let clip = ids[0];
    {
        let item = project.sequence.clip_mut(clip).expect("片段存在");
        item.color.brightness = 20;
        item.color.temperature = -10;
        item.color.lut = Some(PathBuf::from("luts/a.cube"));
        item.keys
            .ensure(crate::timeline::Property::Opacity)
            .insert(Tick::ZERO, 0.0);
        item.keys
            .ensure(crate::timeline::Property::Opacity)
            .insert(s(1), 1.0);
    }
    crate::timeline::add_effect(
        &mut project.sequence,
        clip,
        crate::timeline::EffectKind::Sharpen,
    );

    let plan = plan_sequence(&project).expect("能导出");
    assert_connected(&plan.filter);
    let fade = plan.filter.find("fade=t=in").expect("有淡入");
    let eq = plan.filter.find("eq=brightness=0.200").expect("有亮度");
    let balance = plan.filter.find("colorbalance=rs=-0.030").expect("有色温");
    let lut = plan.filter.find("lut3d='").expect("有 LUT");
    let sharpen = plan.filter.find("unsharp").expect("有锐化");
    // 淡入淡出排在调色之后：先把颜色调好再淡到黑，否则 `fade` 出来的黑会被
    // `eq=brightness` 再提亮一次，画面上根本淡不干净。
    assert!(eq < balance, "{}", plan.filter);
    assert!(balance < lut, "{}", plan.filter);
    assert!(lut < fade, "{}", plan.filter);
    assert!(fade < sharpen, "{}", plan.filter);
    assert!(plan.filter.contains("luts/a.cube"), "{}", plan.filter);
}

/// 调色全默认时不插任何滤镜，链与没加调色时逐字相等。
#[test]
fn 调色为默认值时不进滤镜链() {
    let scratch = Scratch::new("color-neutral");
    let (project, _) = project_with(&scratch, &[(s(0), s(2), Tick::ZERO)]);
    let plan = plan_sequence(&project).expect("能导出");
    assert!(!plan.filter.contains("eq="), "{}", plan.filter);
    assert!(!plan.filter.contains("colorbalance="), "{}", plan.filter);
    assert!(!plan.filter.contains("lut3d="), "{}", plan.filter);
}
