//! Modbus RTU 帧格式一致性测试 — Modbus over Serial Line v1.02
//!
//! 测试 RTU 帧结构、CRC-16 校验、字节顺序。
//!
//! 参考:
//! - Modbus over Serial Line Specification and Implementation Guide v1.02
//! - Modbus Application Protocol Specification v1.1b3

use eneros_device::adapters::modbus_rtu::{
    crc16, encode_rtu_frame, decode_rtu_frame, ModbusRtuError,
};

// ============================================================================
// CRC-16 校验 — Modbus CRC-16-ANSI (多项式 0xA001)
// ============================================================================

/// CRC-16 空输入：初始值 0xFFFF
#[test]
fn test_crc16_empty_input() {
    assert_eq!(crc16(&[]), 0xFFFF);
}

/// CRC-16 标准测试向量："123456789" → 0x4B37
#[test]
fn test_crc16_standard_vector() {
    // CRC-16/Modbus 标准测试向量
    assert_eq!(crc16(b"123456789"), 0x4B37);
}

/// CRC-16 单字节 0x00 → 0x40BF
#[test]
fn test_crc16_single_byte_zero() {
    assert_eq!(crc16(&[0x00]), 0x40BF);
}

/// CRC-16 多项式为 0xA001（反向 0x8005）
#[test]
fn test_crc16_polynomial_is_0xa001() {
    // 验证 CRC-16/Modbus 使用的多项式
    // 通过已知向量间接验证：对 [0x01] 的 CRC
    let crc = crc16(&[0x01]);
    // 手动计算：0xFFFF ^ 0x01 = 0xFFFE
    // 8 次移位，使用多项式 0xA001
    let mut manual: u16 = 0xFFFF;
    manual ^= 0x01;
    for _ in 0..8 {
        if manual & 1 != 0 {
            manual = (manual >> 1) ^ 0xA001;
        } else {
            manual >>= 1;
        }
    }
    assert_eq!(crc, manual);
}

// ============================================================================
// 帧结构 — Modbus RTU
// ============================================================================

/// RTU 帧结构：从站地址(1) + 功能码(1) + 数据(N) + CRC16(2,低字节在前)
#[test]
fn test_rtu_frame_structure() {
    let data = [0x00, 0x0A, 0x00, 0x01]; // 起始地址10, 数量1
    let frame = encode_rtu_frame(0x01, 0x03, &data);

    // 帧长度 = 1(地址) + 1(功能码) + 4(数据) + 2(CRC) = 8
    assert_eq!(frame.len(), 8);
    // 从站地址
    assert_eq!(frame[0], 0x01);
    // 功能码
    assert_eq!(frame[1], 0x03);
    // 数据
    assert_eq!(&frame[2..6], &data);
    // CRC（2 字节）
    assert_eq!(frame.len() - 2, 6); // CRC 前 6 字节
}

/// RTU 帧 CRC 低字节在前（小端序）
#[test]
fn test_rtu_frame_crc_little_endian() {
    let data = [0x02, 0xFF, 0xFF];
    let frame = encode_rtu_frame(0x01, 0x04, &data);
    let expected_crc = crc16(&frame[..frame.len() - 2]);
    // CRC 低字节在前
    assert_eq!(frame[frame.len() - 2], (expected_crc & 0xFF) as u8);
    assert_eq!(frame[frame.len() - 1], (expected_crc >> 8) as u8);
}

/// RTU 帧最小长度：4 字节（地址 + 功能码 + CRC）
#[test]
fn test_rtu_frame_minimum_length() {
    let frame = encode_rtu_frame(0x01, 0x06, &[]);
    assert_eq!(frame.len(), 4); // 1+1+0+2
    let (slave, fc, data) = decode_rtu_frame(&frame).unwrap();
    assert_eq!(slave, 0x01);
    assert_eq!(fc, 0x06);
    assert!(data.is_empty());
}

// ============================================================================
// 字节顺序 — Modbus RTU 大端序
// ============================================================================

/// 寄存器地址/值使用大端序（高字节在前）
#[test]
fn test_rtu_big_endian_address() {
    // FC 0x03 读保持寄存器：起始地址=0x0064(100), 数量=0x0001(1)
    let data = [0x00, 0x64, 0x00, 0x01];
    let frame = encode_rtu_frame(0x01, 0x03, &data);
    let (_, _, pdu) = decode_rtu_frame(&frame).unwrap();

    // 起始地址大端序
    let start_addr = u16::from_be_bytes([pdu[0], pdu[1]]);
    assert_eq!(start_addr, 100);
    // 数量大端序
    let quantity = u16::from_be_bytes([pdu[2], pdu[3]]);
    assert_eq!(quantity, 1);
}

/// 写单线圈值使用大端序：ON=0xFF00, OFF=0x0000
#[test]
fn test_rtu_big_endian_coil_value() {
    // FC 0x05 写单线圈 ON: 地址=0x00AC, 值=0xFF00
    let data = [0x00, 0xAC, 0xFF, 0x00];
    let frame = encode_rtu_frame(0x01, 0x05, &data);
    let (_, _, pdu) = decode_rtu_frame(&frame).unwrap();

    let value = u16::from_be_bytes([pdu[2], pdu[3]]);
    assert_eq!(value, 0xFF00); // ON
}

/// 寄存器值使用大端序
#[test]
fn test_rtu_big_endian_register_value() {
    // FC 0x06 写单寄存器: 地址=0x0001, 值=0x1234
    let data = [0x00, 0x01, 0x12, 0x34];
    let frame = encode_rtu_frame(0x01, 0x06, &data);
    let (_, _, pdu) = decode_rtu_frame(&frame).unwrap();

    let value = u16::from_be_bytes([pdu[2], pdu[3]]);
    assert_eq!(value, 0x1234);
}

// ============================================================================
// 帧编解码往返
// ============================================================================

/// 编码 → 解码往返一致性
#[test]
fn test_rtu_encode_decode_roundtrip() {
    let slave_id = 0x02u8;
    let func_code = 0x03u8;
    let data = [0x00, 0x0A, 0x00, 0x01];

    let frame = encode_rtu_frame(slave_id, func_code, &data);
    let (s, fc, d) = decode_rtu_frame(&frame).unwrap();
    assert_eq!(s, slave_id);
    assert_eq!(fc, func_code);
    assert_eq!(d, data.to_vec());
}

/// 空数据帧编解码往返
#[test]
fn test_rtu_encode_decode_empty_data() {
    let frame = encode_rtu_frame(0x01, 0x06, &[]);
    assert_eq!(frame.len(), 4);
    let (s, fc, d) = decode_rtu_frame(&frame).unwrap();
    assert_eq!(s, 0x01);
    assert_eq!(fc, 0x06);
    assert!(d.is_empty());
}

// ============================================================================
// CRC 校验错误
// ============================================================================

/// CRC 不匹配应返回 CrcMismatch 错误
#[test]
fn test_rtu_decode_crc_mismatch() {
    let mut frame = encode_rtu_frame(0x01, 0x03, &[0x00, 0x01, 0x00, 0x02]);
    // 破坏数据字节
    frame[2] ^= 0xFF;
    let result = decode_rtu_frame(&frame);
    assert!(matches!(result, Err(ModbusRtuError::CrcMismatch(_, _))));
}

/// 直接破坏 CRC 字节
#[test]
fn test_rtu_decode_crc_bytes_flipped() {
    let mut frame = encode_rtu_frame(0x01, 0x03, &[0x00, 0x01]);
    let len = frame.len();
    frame[len - 1] ^= 0xFF;
    let result = decode_rtu_frame(&frame);
    assert!(matches!(result, Err(ModbusRtuError::CrcMismatch(_, _))));
}

/// 帧过短（< 4 字节）应返回错误
#[test]
fn test_rtu_decode_too_short() {
    assert!(decode_rtu_frame(&[0x01, 0x03]).is_err());
    assert!(decode_rtu_frame(&[0x01, 0x03, 0x00]).is_err());
    assert!(decode_rtu_frame(&[]).is_err());
}

// ============================================================================
// CRC-16 更多向量
// ============================================================================

/// CRC-16 向量：完整 RTU 帧
#[test]
fn test_crc16_full_frame_vector() {
    // 从站1, 功能码4(读输入寄存器), 字节计数2, 数据0xFFFF
    let data = [0x01, 0x04, 0x02, 0xFF, 0xFF];
    let crc = crc16(&data);
    // 编码后的帧尾为 CRC 小端序
    let frame = encode_rtu_frame(0x01, 0x04, &[0x02, 0xFF, 0xFF]);
    assert_eq!(frame.len(), 7);
    assert_eq!(&frame[..5], &data);
    assert_eq!(frame[5], (crc & 0xFF) as u8);
    assert_eq!(frame[6], (crc >> 8) as u8);
}

/// CRC-16 向量：双字节
#[test]
fn test_crc16_two_bytes() {
    // 验证 [0x01, 0x03] 的 CRC
    let crc = crc16(&[0x01, 0x03]);
    // 编码并解码验证
    let frame = encode_rtu_frame(0x01, 0x03, &[]);
    assert_eq!(frame.len(), 4);
    assert_eq!(frame[2], (crc & 0xFF) as u8);
    assert_eq!(frame[3], (crc >> 8) as u8);
}
