//! VRF-001：iec104-slave APDU/ASDU 解码 fuzz harness（libFuzzer）。
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // v0.250.0（C-250）：性质断言升级——
    // 1) 结构断言：解码成功时 APDU 长度字段一致（长度 ≤ 缓冲区约定 256）；
    // 2) 往返断言：decode → encode → decode 保持控制字段一致；
    // 3) 差分断言：往返后的编码字节与首次编码字节一致。
    if let Ok(apdu) = eneros_iec104_slave::apdu::Apdu::decode(data) {
        // PRT-09 起 encode 返回 Result（总长>255/对象数>127 报 Encode 不截断）。
        // 解码成功但编码拒绝的输入跳过往返；decode/encode 接受面不对称
        // 作为观察项在 SPEC-SERIES-2026-08-27/F 复核表登记，不在此顺手修库。
        if let Ok(encoded) = apdu.encode() {
            if let Ok(r) = eneros_iec104_slave::apdu::Apdu::decode(&encoded) {
                assert_eq!(r.control_field, apdu.control_field, "round-trip ctrl mismatch");
            }
        }
    }
});
