//! VRF-001：modbus-rtu 帧解析 fuzz harness（libFuzzer）。
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = eneros_modbus_rtu::frame::ModbusFrame::decode(data);

});
