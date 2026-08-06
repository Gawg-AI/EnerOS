//! VRF-001：ipc-wire El0Frame 帧解码 fuzz harness（libFuzzer）。
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = eneros_ipc_wire::el0::decode_frame(data);
});
