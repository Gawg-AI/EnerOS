//! SPEC-GAP-CLOSURE-2026-08-26 / F-2：配置 payload 字段解码 fuzz harness
//! （GAP-FUZZ 三件套之三，面收敛见下注）。
//!
//! 面收敛登记：原计划的 `toml_compile::compile_control_write` 经执行期核对
//! **未在 config-bundle lib.rs 挂载 `pub mod`**——该模块是库内死代码，
//! 外部不可达（预存缺口，随本提交如实上报；补挂载/删除由 owner 卷裁决）。
//! 现打真实公共解析面 `payload::decode_fields`：它是 points/protocol/
//! control_write 各族 wire 的共用底层，crash 覆盖等效且更根本。
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let config = eneros_config_bundle::ConfigId::from_static("fuzz");
    // u8::MAX 作 max_field = 不设字段上限（纯解码纪律扫描）；重复 id /
    // 未知长度 / 越界游标都必须走类型化 ConfigError，panic 即缺陷。
    let _ = eneros_config_bundle::payload::decode_fields(data, &config, u8::MAX);
});
