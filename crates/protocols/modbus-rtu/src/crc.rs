//! CRC-16/MODBUS 校验算法.
//!
//! 多项式 0xA001（即 0x8005 的位反转），初始值 0xFFFF。
//! 用于 Modbus RTU 帧的差错校验。

/// 计算 CRC-16/MODBUS 校验值。
///
/// - 多项式：0xA001（反转多项式）
/// - 初始值：0xFFFF
/// - 输入输出均不反转
///
/// # 示例
///
/// ```
/// use eneros_modbus_rtu::crc16_modbus;
/// // 标准 Modbus 请求 [01 03 00 00 00 01] 的 CRC 值为 0x0A84
/// // （线上字节序为低字节在前：84 0A）
/// assert_eq!(crc16_modbus(&[0x01, 0x03, 0x00, 0x00, 0x00, 0x01]), 0x0A84);
/// // 权威校验值：CRC-16/MODBUS of "123456789" = 0x4B37
/// assert_eq!(crc16_modbus(b"123456789"), 0x4B37);
/// ```
pub fn crc16_modbus(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= byte as u16;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xA001;
            } else {
                crc >>= 1;
            }
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 标准测试向量：读保持寄存器请求帧（不含 CRC）
    #[test]
    fn test_crc_standard_request() {
        // 01 03 00 00 00 01 -> CRC = 0x0A84（线上低字节在前：84 0A）
        let crc = crc16_modbus(&[0x01, 0x03, 0x00, 0x00, 0x00, 0x01]);
        assert_eq!(crc, 0x0A84);
    }

    /// 空输入应返回初始值 0xFFFF
    #[test]
    fn test_crc_empty_input() {
        assert_eq!(crc16_modbus(&[]), 0xFFFF);
    }

    /// 已知测试向量 0xFF 0xFF -> 0x0000
    #[test]
    fn test_crc_ff_ff() {
        assert_eq!(crc16_modbus(&[0xFF, 0xFF]), 0x0000);
    }

    /// 完整帧校验：01 03 00 00 00 01 84 0A（低字节在前）
    #[test]
    fn test_crc_full_frame() {
        let frame = [0x01, 0x03, 0x00, 0x00, 0x00, 0x01, 0x84, 0x0A];
        let crc = crc16_modbus(&frame[..frame.len() - 2]);
        let recv = u16::from_le_bytes([frame[6], frame[7]]);
        assert_eq!(crc, recv);
    }

    /// 单字节测试
    #[test]
    fn test_crc_single_byte() {
        // CRC-16/MODBUS of [0x00]
        assert_eq!(crc16_modbus(&[0x00]), 0x40BF);
    }

    /// CRC-16/MODBUS 标准校验值：对 ASCII "123456789" 应为 0x4B37。
    /// 此为 CRC-16/MODBUS 的权威 check value，用于验证算法正确性。
    #[test]
    fn test_crc_check_value_123456789() {
        let crc = crc16_modbus(b"123456789");
        assert_eq!(crc, 0x4B37);
    }
}
