//! VRF-001：boot-protocol BootInfo/BootParams 解码 fuzz harness（libFuzzer）。
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = eneros_boot_protocol::boot_info::BootInfo::decode(data);
    let _ = eneros_boot_protocol::boot_info::BootParams::decode(data);
});
