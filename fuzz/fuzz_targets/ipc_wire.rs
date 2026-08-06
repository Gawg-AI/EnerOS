//! VRF-001：ipc-wire El0Frame 帧解码 fuzz harness（libFuzzer）。
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // v0.242.0（B6）：解码成功 → 编码往返一致性断言（差分）。
    if let Ok(frame) = eneros_ipc_wire::el0::decode_frame(data) {
        // 解码出的 payload 重新编码，再解码应保持头部一致。
        let encoded = eneros_ipc_wire::el0::encode_frame(
            frame.frame_type,
            frame.payload,
        );
        if let Ok(r) = eneros_ipc_wire::el0::decode_frame(&encoded) {
            assert_eq!(
                r.frame_type, frame.frame_type,
                "round-trip frame_type mismatch"
            );
        }
    }
});
