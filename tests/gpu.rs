//! 显卡枚举：只要保证「不崩、不重复」这两条——具体有几块卡随机器而变，
//! 拿它当断言只会在别人的机器上失败。列不出来（非 Windows）是合法结果。

#[test]
fn 显卡列表不重复且可为空() {
    let adapters = ngy_video_player::platform::gpu::adapters();
    // 打印出来才能在 CI 日志里确认枚举到底读到了什么。
    println!("显卡：{adapters:?}");

    let mut unique = adapters.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), adapters.len(), "枚举结果里有重复项");
    assert!(
        adapters.iter().all(|name| !name.trim().is_empty()),
        "枚举结果里有空名字"
    );
}
