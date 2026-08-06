//! VRF-001：GGUF loader 头/元数据解析 fuzz harness（libFuzzer）。
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok((header, consumed)) = eneros_gguf_loader::header::GgufHeader::parse(data) {
        let _ = eneros_gguf_loader::metadata::GgufMetadata::parse(data, consumed, header.metadata_kv_count);
    }
});
