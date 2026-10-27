//! Windows 文件关联的往返测试。
//!
//! 它会真的写当前用户的注册表（`HKCU`），所以默认 `#[ignore]`，需要手动跑：
//!
//! ```text
//! cargo test --test windows_associations -- --ignored
//! ```
//!
//! 跑完会把注册项删干净，理论上不留痕迹；但别在已经用本程序注册过关联的机器上跑。

#![cfg(windows)]

use ngy_video_player::platform::associations;

#[test]
#[ignore = "会写当前用户的注册表，需要手动执行"]
fn 注册_查询_撤销往返() {
    let expected = associations::count();
    assert!(expected > 0, "可关联格式不该为空");

    let registered = associations::register().expect("注册失败");
    assert_eq!(registered.registered, expected);

    // 反向读一遍：如果写入的键名、类型、路径拼接有任何一处不对，这里就读不回来。
    let status = associations::status();
    assert_eq!(status.registered, expected, "写入的格式数量与读回的不一致");
    assert_eq!(
        status.target,
        std::env::current_exe().ok(),
        "注册项里的路径不是当前可执行文件"
    );

    // 幂等：重复注册不该报错，也不该改变结果。
    associations::register().expect("重复注册失败");
    assert_eq!(associations::status().registered, expected);

    associations::remove().expect("撤销失败");
    assert!(!associations::status().is_registered());
}
