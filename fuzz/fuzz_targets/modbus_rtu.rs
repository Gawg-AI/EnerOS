//! VRF-001：modbus-rtu 帧解析 fuzz harness（libFuzzer）。
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // v0.242.0（B6）：解码成功 → 差分断言（CRC 校验通过则帧结构有效）。
    if let Ok(frame) = eneros_modbus_rtu::frame::ModbusFrame::decode(data) {
        // 重编码后 CRC 必须一致（差分）。
        let reencoded = frame.encode();
        let reparsed = eneros_modbus_rtu::frame::ModbusFrame::decode(&reencoded);
        assert!(reparsed.is_ok(), "round-trip must re-decode");
        if let Ok(r) = reparsed {
            assert_eq!(r.func_code, frame.func_code, "round-trip func mismatch");
            assert_eq!(r.data, frame.data, "round-trip data mismatch");
        }
    }
});
