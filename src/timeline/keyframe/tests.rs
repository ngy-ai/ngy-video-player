use super::*;
use crate::timeline::{Rational, Tick};

fn s(n: f64) -> Tick {
    Tick::from_micros((n * 1_000_000.0) as i64)
}

#[test]
fn 没有点时用默认值() {
    let curve = Animation::new(Property::Scale);
    assert_eq!(curve.value_at(s(1.0)), 1.0);
    assert!(!curve.animated());
}

#[test]
fn 两个点之间线性插值两端钳制() {
    let mut curve = Animation::new(Property::Scale);
    curve.insert(s(0.0), 1.0);
    curve.insert(s(2.0), 2.0);
    assert_eq!(curve.value_at(s(0.0)), 1.0);
    assert_eq!(curve.value_at(s(1.0)), 1.5);
    assert_eq!(curve.value_at(s(2.0)), 2.0);
    // 点外不再外推。
    assert_eq!(curve.value_at(s(5.0)), 2.0);
    assert_eq!(curve.value_at(s(-1.0)), 1.0);
}

#[test]
fn 缓动在中点相同两端更慢() {
    let mut curve = Animation::new(Property::Scale);
    curve.interpolation = Interpolation::EaseInOut;
    curve.insert(s(0.0), 1.0);
    curve.insert(s(2.0), 3.0);
    // smoothstep 在 0.5 处也是 0.5，但在 0.25 处明显低于线性。
    assert!((curve.value_at(s(1.0)) - 2.0).abs() < 1e-5);
    assert!(curve.value_at(s(0.5)) < 1.5);
}

#[test]
fn 同一时刻只留一个点() {
    let mut curve = Animation::new(Property::Scale);
    assert!(curve.insert(s(1.0), 1.2));
    assert!(!curve.insert(s(1.0), 1.8));
    assert_eq!(curve.keys.len(), 1);
    assert_eq!(curve.keys[0].value, 1.8);
}

#[test]
fn 取值被夹在属性的量程内() {
    let mut curve = Animation::new(Property::Opacity);
    curve.insert(s(0.0), 5.0);
    assert_eq!(curve.keys[0].value, 1.0);
    curve.insert(s(1.0), -3.0);
    assert_eq!(curve.keys[1].value, 0.0);
}

#[test]
fn 删点后回到默认值() {
    let mut keys = Keyframes::default();
    keys.ensure(Property::Scale).insert(s(0.0), 1.0);
    keys.ensure(Property::Scale).insert(s(2.0), 2.0);
    assert!(keys.ensure(Property::Scale).remove_near(s(0.0), Tick::ZERO));
    keys.ensure(Property::Scale).clear();
    assert!(keys.is_empty());
    assert_eq!(keys.value(Property::Scale, s(1.0)), 1.0);
}

#[test]
fn 挪标记时整组点一起走() {
    let mut keys = Keyframes::default();
    keys.ensure(Property::Scale).insert(s(1.0), 2.0);
    keys.ensure(Property::Opacity).insert(s(1.0), 0.0);
    keys.ensure(Property::Opacity).insert(s(2.0), 1.0);

    assert!(keys.move_marks(s(1.0), s(3.0)));
    assert_eq!(keys.marks(), vec![s(2.0), s(3.0)]);
    assert_eq!(keys.value(Property::Scale, s(3.0)), 2.0);
    assert_eq!(keys.value(Property::Opacity, s(3.0)), 0.0);

    // 目标时刻已有点：后写的覆盖，同一个时刻只留一个。
    assert!(keys.move_marks(s(3.0), s(2.0)));
    assert_eq!(keys.marks(), vec![s(2.0)]);
    assert_eq!(keys.curve(Property::Scale).unwrap().keys[0].value, 2.0);
    assert_eq!(keys.curve(Property::Opacity).unwrap().keys[0].value, 0.0);

    // 没有点的时刻、以及原地挪，都返回 false。
    assert!(!keys.move_marks(s(0.5), s(1.5)));
    assert!(!keys.move_marks(s(2.0), s(2.0)));
}

#[test]
fn 标记按时间升序去重() {
    let mut keys = Keyframes::default();
    keys.ensure(Property::Scale).insert(s(2.0), 2.0);
    keys.ensure(Property::Scale).insert(s(0.0), 1.0);
    keys.ensure(Property::Opacity).insert(s(0.0), 0.0);
    assert_eq!(keys.marks(), vec![s(0.0), s(2.0)]);
}

#[test]
fn 缩放写成按帧号求值的表达式() {
    let mut keys = Keyframes::default();
    keys.ensure(Property::Scale).insert(s(0.0), 1.0);
    keys.ensure(Property::Scale).insert(s(2.0), 2.0);

    let mut sequence = Sequence::new(
        "测试",
        Rational::new(30, 1).expect("30 fps 合法"),
        1920,
        1080,
    );
    sequence.add_track(crate::timeline::TrackKind::Video);
    let filter = motion_filter(&keys, &sequence);
    assert!(filter.starts_with("zoompan="), "{filter}");
    assert!(filter.contains("s=1920x1080"), "{filter}");
    assert!(filter.contains("fps=30/1"), "{filter}");
    assert!(
        filter.contains("(on/30.000000)"),
        "用帧号换算时间：{filter}"
    );
    assert!(filter.contains("if(lt("), "分段表达式：{filter}");
}

#[test]
fn 没有缩放或位置时不出动画滤镜() {
    let mut sequence = Sequence::new("测试", Rational::new(30, 1).expect("30 fps 合法"), 640, 360);
    sequence.add_track(crate::timeline::TrackKind::Video);
    let keys = Keyframes::default();
    assert_eq!(motion_filter(&keys, &sequence), "");

    // 只有不透明度时也不该插 zoompan。
    let mut keys = Keyframes::default();
    keys.ensure(Property::Opacity).insert(s(0.0), 0.0);
    keys.ensure(Property::Opacity).insert(s(1.0), 1.0);
    assert_eq!(motion_filter(&keys, &sequence), "");
}

#[test]
fn 不透明度写成淡入淡出() {
    let mut keys = Keyframes::default();
    keys.ensure(Property::Opacity).insert(s(0.5), 0.0);
    keys.ensure(Property::Opacity).insert(s(1.5), 1.0);
    let filters = fade_filters(&keys, s(3.0));
    assert_eq!(
        filters,
        vec!["fade=t=in:st=0.500000:d=1.000000".to_string()]
    );

    // 中间值 fade 表达不了：宁可不做。
    let mut keys = Keyframes::default();
    keys.ensure(Property::Opacity).insert(s(0.0), 0.3);
    keys.ensure(Property::Opacity).insert(s(1.0), 0.8);
    assert!(fade_filters(&keys, s(3.0)).is_empty());
}

#[test]
fn 认不出来的属性名落进未知() {
    let property: Property = "hue".to_string().into();
    assert_eq!(property, Property::Unknown);
    assert_eq!(String::from(Property::Scale), "scale");
}

#[test]
fn 贝塞尔缓动端点正确且单调() {
    let easing = Interpolation::Bezier {
        x1: 0.42,
        y1: 0.0,
        x2: 0.58,
        y2: 1.0,
    };
    assert!(easing.ease(0.0).abs() < 1e-4);
    assert!((easing.ease(1.0) - 1.0).abs() < 1e-4);
    let mut previous = 0.0;
    for step in 0..=20 {
        let value = easing.ease(step as f32 / 20.0);
        assert!(value >= previous - 1e-4, "不该回头：{value} < {previous}");
        previous = value;
    }
    // 对称控制点的中点该在 0.5 附近，差太多说明解错了 t。
    assert!(
        (easing.ease(0.5) - 0.5).abs() < 0.05,
        "中点 {}",
        easing.ease(0.5)
    );
}

#[test]
fn 控制点贴边也不发散() {
    for (x1, y1, x2, y2) in [
        (0.0, 0.0, 0.0, 0.0),
        (1.0, 1.0, 1.0, 1.0),
        (0.0, 1.0, 0.0, 0.0),
        (1.0, 0.0, 1.0, 1.0),
    ] {
        let easing = Interpolation::with_controls(x1, y1, x2, y2);
        for step in 0..=10 {
            let value = easing.ease(step as f32 / 10.0);
            assert!(
                (0.0..=1.0).contains(&value),
                "{x1},{y1},{x2},{y2} -> {value}"
            );
        }
    }
}

#[test]
fn 直线控制点等价于线性() {
    let easing = Interpolation::with_controls(1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0);
    for step in 0..=10 {
        let x = step as f32 / 10.0;
        assert!(
            (easing.ease(x) - x).abs() < 2e-3,
            "{x} -> {}",
            easing.ease(x)
        );
    }
}

#[test]
fn 控制点被夹进合法范围() {
    let easing = Interpolation::with_controls(-1.0, 2.0, 3.0, -1.0);
    assert_eq!(
        easing,
        Interpolation::Bezier {
            x1: 0.0,
            y1: 1.0,
            x2: 1.0,
            y2: 0.0
        }
    );
}

#[test]
fn 每个点自己的缓动盖过曲线级插值() {
    let mut curve = Animation::new(Property::Scale);
    curve.interpolation = Interpolation::EaseInOut;
    curve.insert(s(0.0), 1.0);
    curve.insert(s(1.0), 2.0);
    // 点上没有 easing：走曲线级的 smoothstep，0.25 处是 1 + 0.25²(3-0.5)。
    assert!((curve.value_at(s(0.25)) - 1.15625).abs() < 1e-3);
    // 起点这一段改成线性：0.25 处该是 1.25。
    curve.keys[0].easing = Some(Interpolation::Linear);
    assert!((curve.value_at(s(0.25)) - 1.25).abs() < 1e-3);
}

#[test]
fn 老工程没有_easing_字段也能读() {
    let text = r#"{"property":"scale","interpolation":"ease_in_out","keys":[{"at":0,"value":1.0},{"at":1000000,"value":2.0}]}"#;
    let curve: Animation = serde_json::from_str(text).expect("老工程要能读");
    assert_eq!(curve.interpolation, Interpolation::EaseInOut);
    assert!(curve.keys.iter().all(|key| key.easing.is_none()));
    assert!((curve.value_at(s(0.5)) - 1.5).abs() < 1e-3);
}

#[test]
fn 贝塞尔的导出表达式切成了八段折线() {
    let from = Keyframe {
        at: s(0.0),
        value: 1.0,
        easing: Some(Interpolation::Bezier {
            x1: 0.42,
            y1: 0.0,
            x2: 0.58,
            y2: 1.0,
        }),
    };
    let to = Keyframe::new(s(2.0), 2.0);
    let text = segment_expr(&from, &to, 2.0, Interpolation::Linear, "(on/25)");
    assert_eq!(text.matches("if(lt(").count(), BEZIER_STEPS);
    // 末段兜底回到 `to.value`，否则最后一段会漂。
    assert!(text.contains(",2.000000)"), "{text}");
}

#[test]
fn 线性段的表达式还是老样子() {
    let from = Keyframe::new(s(0.0), 1.0);
    let to = Keyframe::new(s(2.0), 3.0);
    let text = segment_expr(&from, &to, 2.0, Interpolation::Linear, "(on/25)");
    assert_eq!(text, "1.000000+(2.000000)*(((on/25)-0.000000)/2.000000)");
}
