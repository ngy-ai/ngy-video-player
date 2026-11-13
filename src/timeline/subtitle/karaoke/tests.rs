//! 逐字时间轴的纯计算单测：权重修整、边界、播放头定位、厘秒分配。

use super::*;

#[test]
fn 换行不占节拍且关掉时不会被文字改动打开() {
    assert_eq!(visible_count("第一行\n第二行"), 6);
    assert!(fit(&[], "新字幕").is_empty());

    let mut fitted = fit(&[100, 200, 300], "一\n二");
    assert_eq!(fitted, vec![100, 200], "裁掉多出来的权重");
    fitted = fit(&[100, 200], "一二三四");
    assert_eq!(fitted, vec![100, 200, 200, 200], "新字沿用最后一字的权重");
    assert_eq!(fit(&[0, 0], "ab"), vec![1, 1], "权重不许是 0");
}

#[test]
fn 边界严格收在总时长上且按权重分() {
    let weights = [1, 3];
    let bounds = offsets(&weights, Tick::from_secs(4.0));
    assert_eq!(bounds[0], Tick::from_secs(1.0));
    assert_eq!(bounds[1], Tick::from_secs(4.0), "最后一段钉死在总时长");
    assert!(offsets(&[], Tick::from_secs(4.0)).is_empty());

    // 除不尽的余数不该让最后一个字提前收尾。
    let uneven = offsets(&[1, 1, 1], Tick::from_micros(10));
    assert_eq!(uneven.len(), 3);
    assert_eq!(*uneven.last().unwrap(), Tick::from_micros(10));
    assert_eq!(uneven[0], Tick::from_micros(3));
    assert_eq!(uneven[1], Tick::from_micros(6));
}

#[test]
fn 播放头按边界落在对应的字上() {
    let bounds = offsets(&[1, 1, 1], Tick::from_secs(3.0));
    assert_eq!(index_at(&bounds, Tick::from_micros(-1)), None);
    assert_eq!(index_at(&bounds, Tick::ZERO), Some(0));
    assert_eq!(index_at(&bounds, Tick::from_micros(999_999)), Some(0));
    assert_eq!(index_at(&bounds, Tick::from_secs(1.0)), Some(1));
    assert_eq!(index_at(&bounds, Tick::from_secs(2.5)), Some(2));
    assert_eq!(index_at(&bounds, Tick::from_secs(3.0)), None, "结尾不含");
    assert_eq!(index_at(&[], Tick::ZERO), None);
}

#[test]
fn 预览分段换行不占节拍且拼回去还是原文字() {
    assert_eq!(split("甲乙丙", 0), ("".to_string(), "甲乙丙".to_string()));
    assert_eq!(split("甲乙丙", 2), ("甲乙".to_string(), "丙".to_string()));
    assert_eq!(split("甲乙丙", 9), ("甲乙丙".to_string(), "".to_string()));
    let (done, rest) = split("第一行\n第二行", 3);
    assert_eq!(format!("{done}{rest}"), "第一行 第二行");
    assert_eq!(done, "第一行");
    assert_eq!(rest, " 第二行", "换行压成空格留在后一段");
}

#[test]
fn 拖分界线只重切相邻两个字() {
    let weights = [100, 100, 100];
    let (left, right) = move_boundary(&weights, 0, 0.25).unwrap();
    assert_eq!((left, right), (50, 150), "往前拖 1/4：前字变短、后字变长");
    assert_eq!(left + right, 200);
    assert_eq!(weights[2], 100, "第三个字不动");
    assert_eq!(move_boundary(&weights, 2, 0.5), None);
    assert_eq!(move_boundary(&weights, 0, f32::NAN).unwrap(), (100, 100));
    assert_eq!(
        move_boundary(&[1, 1], 0, 0.9).unwrap(),
        (1, 1),
        "和只有 2 时两边都留 1"
    );
    assert_eq!(move_boundary(&weights, 0, 9.0).unwrap(), (199, 1));
}

#[test]
fn 厘秒分配总和严格等于_dialogue_时长() {
    assert_eq!(centis(&[1, 1, 1], 10), vec![4, 3, 3], "余数补给靠前的字");
    assert_eq!(centis(&[1, 3], 40), vec![10, 30]);
    assert_eq!(
        centis(&[1, 1, 1, 1], 2),
        vec![1, 1, 0, 0],
        "字比厘秒多时允许 0"
    );
    assert_eq!(centis(&[], 25), Vec::<i64>::new());
    for total in 0..40 {
        let parts = centis(&[7, 3, 1, 9], total);
        assert_eq!(parts.iter().sum::<i64>(), total, "total={total}");
        assert!(parts.iter().all(|part| *part >= 0));
    }
}
