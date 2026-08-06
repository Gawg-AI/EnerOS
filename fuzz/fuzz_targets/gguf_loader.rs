//! VRF-001：GGUF loader 头/元数据解析 fuzz harness（libFuzzer）。
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok((header, consumed)) = eneros_gguf_loader::header::GgufHeader::parse(data) {
        // v0.250.0（C-250）：性质断言——
        // 1) 结构断言：magic 与版本字段不变量；
        // 2) 结构断言：metadata_kv_count 有界（防止解析器被超大计数拖垮）；
        // 3) 结构断言：consumed ≤ data.len()（解析不得越界声明）。
        assert!(header.magic == eneros_gguf_loader::header::GGUF_MAGIC, "magic mismatch");
        assert!(header.metadata_kv_count < 1_000_000, "kv_count unbounded");
        assert!(consumed <= data.len(), "consumed exceeds input");
        let _ = eneros_gguf_loader::metadata::GgufMetadata::parse(data, consumed, header.metadata_kv_count);
    }
});
