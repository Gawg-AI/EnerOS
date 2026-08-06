//! VRF-001：iec104-slave APDU/ASDU 解码 fuzz harness（libFuzzer）。
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = eneros_iec104_slave::apdu::Apdu::decode(data);
});
