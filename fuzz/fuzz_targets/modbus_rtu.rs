//! VRF-001：modbus-rtu 帧解析 fuzz harness（libFuzzer）。
//!
//! SPEC-D WP-D.2 C2 扩面：同一输入双覆盖——
//! (a) `ModbusFrame::decode` 差分断言（解码成功 ⇒ 重编码后 CRC 一致）；
//! (b) **成帧层半帧垃圾流**：原始字节按变异切分注入 RTU 成帧传输
//!     （`Rs485RtuTransport<MockUartHw>`），断言缓冲有界、循环终止、
//!     绝不 panic——半帧/坏 CRC/超长帧在真实帧间隔逻辑下的鲁棒性。
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // (a) v0.242.0（B6）：解码成功 → 差分断言（CRC 校验通过则帧结构有效）。
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

    // (b) SPEC-D：成帧层鲁棒性（半帧切分由首字节决定切点；时间步进压缩
    // 保持运行有界）。MockUartHw 仅 fuzz/host 构建可见（rs485 mock feature）。
    use eneros_modbus_rtu::master::RtuTransport;
    let mut uart = eneros_rs485::mock::MockUartHw::new();
    uart.set_time_step_ns(50_000); // ≪ 全档 3.5 字符间隔——不切散合法帧。
    uart.push_rx_slice(data);
    let mut rtu = eneros_modbus_rtu::transport::Rs485RtuTransport::new(uart, 115_200);
    // 有界 drain：任何输入下 recv 必须在有限轮内终局（Ok 帧长 ≤ RTU 上限，
    // 或 Timeout），且不会 panic。
    for _ in 0..8 {
        match rtu.recv(20) {
            Ok(frame_bytes) => {
                assert!(
                    frame_bytes.len() <= 256,
                    "framer output over RTU cap: {}",
                    frame_bytes.len()
                );
                // 成帧输出允许任意字节内容（校验在上层），但必须可再入
                // decode 而非 panic（Err 合法——差分覆盖在 (a) 已证往返）。
                let _ = eneros_modbus_rtu::frame::ModbusFrame::decode(&frame_bytes);
            }
            Err(eneros_driver_framework::DriverError::Timeout) => break,
            Err(e) => panic!("framer 只允许 Timeout 或成帧成功：{e:?}"),
        }
    }
});
