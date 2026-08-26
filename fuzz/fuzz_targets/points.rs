//! SPEC-GAP-CLOSURE-2026-08-26 / F-2：点表 wire 解码 fuzz harness（GAP-FUZZ
//! 三件套之二）。EIPC 明文入口的最外层解析面——crash 扫描优先。
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let config = eneros_config_bundle::ConfigId::from_static("fuzz");
    // 任意字节 × 固定配置 id：解码失败必须走类型化错误，不允许 panic。
    // （结构性质断言随 KAT 卷补强——本目标首发覆盖 crash 面与错误枚举纪律。）
    let _ = eneros_config_bundle::points::decode_points(data, &config);
});
