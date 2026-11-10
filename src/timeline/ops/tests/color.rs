//! 调色（M9-T11）：整套参数一次换，锁轨与未知片段与其它编辑一样拒绝。
//!
//! 滤镜映射本身在 `timeline/color.rs`，导出链的顺序在 `export/tests.rs`；
//! 这里只钉「操作层」：值有没有落上去、没变时会不会多一步撤销。

use super::*;

fn grade(brightness: i32) -> ColorGrade {
    ColorGrade {
        brightness,
        ..ColorGrade::default()
    }
}

#[test]
fn 调色参数落到片段上() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let id = push(&mut sequence, track, s(0), s(2), s(0));

    let next = applied(
        &sequence,
        EditOp::SetColor {
            clip: id,
            color: ColorGrade {
                brightness: 20,
                contrast: -10,
                saturation: 5,
                temperature: 30,
                lut: Some("luts/a.cube".into()),
            },
        },
    );
    let color = &next.clip(id).unwrap().color;
    assert_eq!(color.brightness, 20);
    assert_eq!(color.contrast, -10);
    assert_eq!(color.saturation, 5);
    assert_eq!(color.temperature, 30);
    assert_eq!(
        color.lut.as_deref(),
        Some(std::path::Path::new("luts/a.cube"))
    );
}

#[test]
fn 值没变时序列原样返回() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let id = push(&mut sequence, track, s(0), s(2), s(0));

    // 片段本来就是默认调色：再设一次默认值不该多出一步撤销。
    let same = apply(
        &sequence,
        &EditOp::SetColor {
            clip: id,
            color: ColorGrade::default(),
        },
    )
    .unwrap();
    assert_eq!(same, sequence);

    // 改过一次之后再还原，应当回到默认。
    let changed = applied(
        &sequence,
        EditOp::SetColor {
            clip: id,
            color: grade(20),
        },
    );
    assert_eq!(changed.clip(id).unwrap().color.brightness, 20);
    let back = applied(
        &changed,
        EditOp::SetColor {
            clip: id,
            color: ColorGrade::default(),
        },
    );
    assert_eq!(back, sequence);
}

#[test]
fn 锁定轨道与未知片段都拒绝() {
    let mut sequence = seq();
    let track = sequence.tracks[0].id;
    let id = push(&mut sequence, track, s(0), s(2), s(0));
    sequence.track_mut(track).unwrap().locked = true;

    assert_eq!(
        apply(
            &sequence,
            &EditOp::SetColor {
                clip: id,
                color: grade(10)
            }
        ),
        Err(EditError::TrackLocked(track))
    );

    sequence.track_mut(track).unwrap().locked = false;
    assert_eq!(
        apply(
            &sequence,
            &EditOp::SetColor {
                clip: Id::new(999),
                color: grade(10)
            }
        ),
        Err(EditError::UnknownClip(Id::new(999)))
    );
}
