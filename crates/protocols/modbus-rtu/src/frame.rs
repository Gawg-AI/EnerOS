//! Modbus RTU 帧结构 — 地址 + 功能码 + 数据 + CRC.

use alloc::vec::Vec;

use crate::crc::crc16_modbus;
use crate::error::ModbusError;

/// Modbus RTU 帧
///
/// 结构：`[slave_addr(1)][func_code(1)][data(...)][crc_le(2)]`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModbusFrame {
    /// 从站地址
    pub slave_addr: u8,
    /// 功能码
    pub func_code: u8,
    /// 数据域
    pub data: Vec<u8>,
    /// CRC 校验值（小端存储于线上）
    pub crc: u16,
}

impl ModbusFrame {
    /// 构造新帧并自动计算 CRC。
    ///
    /// CRC 计算范围：`[slave_addr][func_code][data]`
    pub fn new(slave_addr: u8, func_code: u8, data: Vec<u8>) -> Self {
        let mut crc_buf = Vec::with_capacity(data.len() + 2);
        crc_buf.push(slave_addr);
        crc_buf.push(func_code);
        crc_buf.extend_from_slice(&data);
        let crc = crc16_modbus(&crc_buf);
        Self {
            slave_addr,
            func_code,
            data,
            crc,
        }
    }

    /// 编码为线上字节流（小端 CRC）。
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.data.len() + 4);
        buf.push(self.slave_addr);
        buf.push(self.func_code);
        buf.extend_from_slice(&self.data);
        buf.extend_from_slice(&self.crc.to_le_bytes());
        buf
    }

    /// 从字节流解码帧（校验 CRC）。
    ///
    /// 最小帧长 4 字节（addr + func + crc_lo + crc_hi）。
    pub fn decode(buf: &[u8]) -> Result<Self, ModbusError> {
        if buf.len() < 4 {
            return Err(ModbusError::FrameTooShort);
        }
        let crc_recv = u16::from_le_bytes([buf[buf.len() - 2], buf[buf.len() - 1]]);
        let crc_calc = crc16_modbus(&buf[..buf.len() - 2]);
        if crc_recv != crc_calc {
            return Err(ModbusError::CrcMismatch);
        }
        Ok(Self {
            slave_addr: buf[0],
            func_code: buf[1],
            data: buf[2..buf.len() - 2].to_vec(),
            crc: crc_recv,
        })
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    #[test]
    fn test_encode_decode_round_trip() {
        let frame = ModbusFrame::new(0x01, 0x03, vec![0x00, 0x00, 0x00, 0x01]);
        let encoded = frame.encode();
        assert_eq!(encoded.len(), 8); // 2 + 4 + 2

        let decoded = ModbusFrame::decode(&encoded).expect("decode should succeed");
        assert_eq!(decoded.slave_addr, 0x01);
        assert_eq!(decoded.func_code, 0x03);
        assert_eq!(decoded.data, vec![0x00, 0x00, 0x00, 0x01]);
        assert_eq!(decoded.crc, frame.crc);
    }

    #[test]
    fn test_encode_known_frame() {
        // 01 03 00 00 00 01 -> CRC 0x0A84 -> 线上低字节在前 84 0A
        let frame = ModbusFrame::new(0x01, 0x03, vec![0x00, 0x00, 0x00, 0x01]);
        let encoded = frame.encode();
        assert_eq!(
            encoded,
            vec![0x01, 0x03, 0x00, 0x00, 0x00, 0x01, 0x84, 0x0A]
        );
    }

    #[test]
    fn test_decode_too_short() {
        assert_eq!(
            ModbusFrame::decode(&[0x01, 0x03]),
            Err(ModbusError::FrameTooShort)
        );
        assert_eq!(
            ModbusFrame::decode(&[0x01, 0x03, 0x0A]),
            Err(ModbusError::FrameTooShort)
        );
    }

    #[test]
    fn test_decode_crc_mismatch() {
        // 故意破坏 CRC
        let bad = [0x01, 0x03, 0x00, 0x00, 0x00, 0x01, 0xFF, 0xFF];
        assert_eq!(ModbusFrame::decode(&bad), Err(ModbusError::CrcMismatch));
    }

    #[test]
    fn test_empty_data_frame() {
        let frame = ModbusFrame::new(0x01, 0x03, Vec::new());
        let encoded = frame.encode();
        assert_eq!(encoded.len(), 4); // addr + func + crc(2)
        let decoded = ModbusFrame::decode(&encoded).expect("decode should succeed");
        assert_eq!(decoded.data, Vec::new());
        assert_eq!(decoded.slave_addr, 0x01);
        assert_eq!(decoded.func_code, 0x03);
    }

    #[test]
    fn test_broadcast_addr_zero() {
        let frame = ModbusFrame::new(0x00, 0x10, vec![0x00, 0x01, 0x02, 0x00, 0x0A]);
        let encoded = frame.encode();
        let decoded = ModbusFrame::decode(&encoded).expect("decode broadcast");
        assert_eq!(decoded.slave_addr, 0x00);
    }
}
