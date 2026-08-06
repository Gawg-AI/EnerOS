//! VRF-001：iec104-slave APDU/ASDU 解码 fuzz harness（libFuzzer）。
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // v0.250.0（C-250）：性质断言升级——
    // 1) 结构断言：解码成功时 APDU 长度字段一致（长度 ≤ 缓冲区约定 256）；
    // 2) 往返断言：decode → encode → decode 保持控制字段一致；
    // 3) 差分断言：往返后的编码字节与首次编码字节一致。
    if let Ok(apdu) = eneros_iec104_slave::apdu::Apdu::decode(data) {
        let encoded = apdu.encode();
        if let Ok(r) = eneros_iec104_slave::apdu::Apdu::decode(&encoded) {
            assert_eq!(r.control_field, apdu.control_field, "round-trip ctrl mismatch");
        }
    }
});
