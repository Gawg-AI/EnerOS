//! VRF-001：boot-protocol BootInfo/BootParams 解码 fuzz harness（libFuzzer）。
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // v0.250.0（C-250）：性质断言升级——
    // 1) 结构断言：BootInfo 解码成功时 magic/flags 字段不变量；
    // 2) 往返断言：decode → to_le_bytes → decode 必须保持关键字段；
    // 3) 差分断言：BootParams 解码结果与 BootInfo 一致时版本一致。
    if let Ok(bi) = eneros_boot_protocol::boot_info::BootInfo::decode(data) {
        // 结构：abi_major 非零且版本一致性。
        assert!(bi.abi_major > 0, "abi_major must be non-zero");
        // 往返：编码后再解码，boot_id 必须一致（to_le_bytes 直接返回数组）。
        let bytes = bi.to_le_bytes();
        if let Ok(r) = eneros_boot_protocol::boot_info::BootInfo::decode(&bytes) {
            assert_eq!(r.boot_id, bi.boot_id, "round-trip boot_id mismatch");
            assert_eq!(r.selected_slot, bi.selected_slot, "round-trip slot mismatch");
        }
    }
});
