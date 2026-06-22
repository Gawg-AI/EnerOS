//! Modbus 异常码响应一致性测试 — Modbus Application Protocol v1.1b3 §7
//!
//! 测试异常响应格式和异常码值。
//!
//! 异常响应格式：
//!   功能码 | 0x80（最高位置 1）+ 异常码(1B)
//!
//! 参考:
//! - Modbus Application Protocol Specification v1.1b3 §7: Exception Responses

use eneros_device::adapters::modbus_rtu::{
    encode_rtu_frame, decode_rtu_frame, ModbusRtuError,
};

// ============================================================================
// 异常响应格式 — Modbus 协议 §7
// ============================================================================

/// 异常响应：功能码最高位置 1（function_code | 0x80）
#[test]
fn test_exception_response_function_code_high_bit() {
    // 正常功能码 0x03 → 异常功能码 0x83
    let normal_fc: u8 = 0x03;
    let exception_fc: u8 = normal_fc | 0x80;
    assert_eq!(exception_fc, 0x83);
}

/// 异常响应帧结构：从站地址 + 异常功能码 + 异常码 + CRC
#[test]
fn test_exception_response_frame_structure() {
    // 异常码 0x02（非法数据地址）
    let frame = encode_rtu_frame(0x01, 0x83, &[0x02]);
    let (slave, fc, data) = decode_rtu_frame(&frame).unwrap();
    assert_eq!(slave, 0x01);
    assert_eq!(fc, 0x83); // 0x03 | 0x80
    assert_eq!(data, vec![0x02]); // 异常码
    assert!(fc & 0x80 != 0); // 异常标志
}

// ============================================================================
// 异常码 0x01 — 非法功能码 (Illegal Function)
// ============================================================================

/// 异常码 0x01：接收到的功能码不支持
#[test]
fn test_exception_code_0x01_illegal_function() {
    const ILLEGAL_FUNCTION: u8 = 0x01;
    assert_eq!(ILLEGAL_FUNCTION, 0x01);

    // 构造异常响应帧：FC=0x91(0x11|0x80), 异常码=0x01
    let frame = encode_rtu_frame(0x01, 0x91, &[0x01]);
    let (slave, fc, data) = decode_rtu_frame(&frame).unwrap();
    assert_eq!(slave, 0x01);
    assert_eq!(fc, 0x91);
    assert_eq!(data[0], ILLEGAL_FUNCTION);
}

// ============================================================================
// 异常码 0x02 — 非法数据地址 (Illegal Data Address)
// ============================================================================

/// 异常码 0x02：数据地址超出范围
#[test]
fn test_exception_code_0x02_illegal_data_address() {
    const ILLEGAL_DATA_ADDRESS: u8 = 0x02;
    assert_eq!(ILLEGAL_DATA_ADDRESS, 0x02);

    // FC=0x83(0x03|0x80), 异常码=0x02
    let frame = encode_rtu_frame(0x01, 0x83, &[0x02]);
    let (_, fc, data) = decode_rtu_frame(&frame).unwrap();
    assert_eq!(fc & 0x7F, 0x03); // 原始功能码
    assert_eq!(data[0], ILLEGAL_DATA_ADDRESS);
}

// ============================================================================
// 异常码 0x03 — 非法数据值 (Illegal Data Value)
// ============================================================================

/// 异常码 0x03：数据值超出范围
#[test]
fn test_exception_code_0x03_illegal_data_value() {
    const ILLEGAL_DATA_VALUE: u8 = 0x03;
    assert_eq!(ILLEGAL_DATA_VALUE, 0x03);

    // FC=0x86(0x06|0x80), 异常码=0x03
    let frame = encode_rtu_frame(0x01, 0x86, &[0x03]);
    let (_, fc, data) = decode_rtu_frame(&frame).unwrap();
    assert_eq!(fc & 0x7F, 0x06);
    assert_eq!(data[0], ILLEGAL_DATA_VALUE);
}

// ============================================================================
// 异常码 0x04 — 从站设备故障 (Slave Device Failure)
// ============================================================================

/// 异常码 0x04：从站设备故障
#[test]
fn test_exception_code_0x04_slave_device_failure() {
    const SLAVE_DEVICE_FAILURE: u8 = 0x04;
    assert_eq!(SLAVE_DEVICE_FAILURE, 0x04);

    // FC=0x84(0x04|0x80), 异常码=0x04
    let frame = encode_rtu_frame(0x01, 0x84, &[0x04]);
    let (_, fc, data) = decode_rtu_frame(&frame).unwrap();
    assert_eq!(fc & 0x7F, 0x04);
    assert_eq!(data[0], SLAVE_DEVICE_FAILURE);
}

// ============================================================================
// 异常码 0x06 — 从站设备忙 (Slave Device Busy)
// ============================================================================

/// 异常码 0x06：从站设备忙
#[test]
fn test_exception_code_0x06_slave_device_busy() {
    const SLAVE_DEVICE_BUSY: u8 = 0x06;
    assert_eq!(SLAVE_DEVICE_BUSY, 0x06);

    // FC=0x86(0x06|0x80), 异常码=0x06
    let frame = encode_rtu_frame(0x01, 0x86, &[0x06]);
    let (_, fc, data) = decode_rtu_frame(&frame).unwrap();
    assert_eq!(fc & 0x7F, 0x06);
    assert_eq!(data[0], SLAVE_DEVICE_BUSY);
}

// ============================================================================
// 异常响应验证
// ============================================================================

/// 所有标准异常码（0x01-0x04, 0x06）均可正确编解码
#[test]
fn test_all_standard_exception_codes() {
    let exception_codes = [
        (0x01, "非法功能码"),
        (0x02, "非法数据地址"),
        (0x03, "非法数据值"),
        (0x04, "从站设备故障"),
        (0x06, "从站设备忙"),
    ];

    for (code, _name) in &exception_codes {
        // 使用功能码 0x03 的异常响应
        let frame = encode_rtu_frame(0x01, 0x83, &[*code]);
        let (slave, fc, data) = decode_rtu_frame(&frame).unwrap();
        assert_eq!(slave, 0x01);
        assert_eq!(fc, 0x83); // 异常功能码
        assert!(fc & 0x80 != 0); // 异常标志
        assert_eq!(data[0], *code);
    }
}

/// 异常响应帧长度：从站(1) + 异常功能码(1) + 异常码(1) + CRC(2) = 5 字节
#[test]
fn test_exception_response_frame_length() {
    let frame = encode_rtu_frame(0x01, 0x83, &[0x02]);
    assert_eq!(frame.len(), 5);
}

/// 异常响应 CRC 校验正确
#[test]
fn test_exception_response_crc_valid() {
    let frame = encode_rtu_frame(0x01, 0x83, &[0x02]);
    // decode_rtu_frame 内部校验 CRC，成功表示 CRC 正确
    let result = decode_rtu_frame(&frame);
    assert!(result.is_ok());
}

/// 异常响应 CRC 错误应被检测
#[test]
fn test_exception_response_crc_error() {
    let mut frame = encode_rtu_frame(0x01, 0x83, &[0x02]);
    frame[3] ^= 0xFF; // 破坏异常码字节
    let result = decode_rtu_frame(&frame);
    assert!(matches!(result, Err(ModbusRtuError::CrcMismatch(_, _))));
}

// ============================================================================
// 异常功能码映射
// ============================================================================

/// 每个标准功能码的异常功能码 = 原始功能码 | 0x80
#[test]
fn test_exception_function_code_mapping() {
    let test_cases = [
        (0x01u8, 0x81u8), // Read Coils
        (0x02, 0x82),     // Read Discrete Inputs
        (0x03, 0x83),     // Read Holding Registers
        (0x04, 0x84),     // Read Input Registers
        (0x05, 0x85),     // Write Single Coil
        (0x06, 0x86),     // Write Single Register
        (0x0F, 0x8F),     // Write Multiple Coils
        (0x10, 0x90),     // Write Multiple Registers
    ];

    for (normal, exception) in &test_cases {
        assert_eq!(*normal | 0x80, *exception);
    }
}
