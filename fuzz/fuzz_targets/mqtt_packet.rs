//! SPEC-GAP-CLOSURE-2026-08-26 / F-2：MQTT 帧解码 fuzz harness（libFuzzer）。
//! GAP-FUZZ 清账三件套之一；CI runner 接线另计（52 卷登记口径）。
#![no_main]
use libfuzzer_sys::fuzz_target;

use eneros_mqtt::packet;

fuzz_target!(|data: &[u8]| {
    // 性质 1：任意字节流解码不得 panic（错误分类随意，崩溃即 bug）。
    if let Ok(pkt) = packet::decode(data) {
        // 性质 2：两个定长无载荷控制帧的编码是常量（往返自证）。
        match pkt {
            eneros_mqtt::packet::MqttPacket::Pingreq => {
                assert_eq!(packet::encode(&pkt), vec![0xC0, 0x00]);
            }
            eneros_mqtt::packet::MqttPacket::Disconnect => {
                assert_eq!(packet::encode(&pkt), vec![0xE0, 0x00]);
            }
            _ => {}
        }
    }
});
