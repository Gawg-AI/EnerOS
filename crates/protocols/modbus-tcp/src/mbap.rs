//! MBAP（Modbus Application Protocol）头部编解码.
//!
//! MBAP 头部 7 字节，大端序：
//! - transaction_id (u16)：事务 ID，请求/响应配对
//! - protocol_id (u16)：协议 ID，Modbus = 0
//! - length (u16)：后续字节数（unit_id + PDU）
//! - unit_id (u8)：单元 ID（对应 RTU 的从站地址）

use crate::error::ModbusTcpError;

/// MBAP 头部（7 字节）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MbapHeader {
    /// 事务 ID
    pub transaction_id: u16,
    /// 协议 ID（Modbus = 0）
    pub protocol_id: u16,
    /// 后续字节数（unit_id + PDU）
    pub length: u16,
    /// 单元 ID
    pub unit_id: u8,
}

impl MbapHeader {
    /// 创建 MBAP 头部。
    ///
    /// - `protocol_id` 固定为 0（Modbus 协议）
    /// - `length` = `data_len`（PDU 字节数） + 1（unit_id 占 1 字节）
    pub fn new(transaction_id: u16, unit_id: u8, data_len: u16) -> Self {
        Self {
            transaction_id,
            protocol_id: 0,
            length: data_len + 1,
            unit_id,
        }
    }

    /// 编码为 7 字节大端序数组。
    pub fn encode(&self) -> [u8; 7] {
        let txn = self.transaction_id.to_be_bytes();
        let proto = self.protocol_id.to_be_bytes();
        let len = self.length.to_be_bytes();
        [
            txn[0],
            txn[1], // transaction_id
            proto[0],
            proto[1], // protocol_id
            len[0],
            len[1],       // length
            self.unit_id, // unit_id
        ]
    }

    /// 从字节缓冲解码 MBAP 头部。
    ///
    /// - 缓冲长度 < 7 → `FrameTooShort`
    /// - protocol_id != 0 → `InvalidProtocolId`
    pub fn decode(buf: &[u8]) -> Result<Self, ModbusTcpError> {
        if buf.len() < 7 {
            return Err(ModbusTcpError::FrameTooShort);
        }
        let transaction_id = u16::from_be_bytes([buf[0], buf[1]]);
        let protocol_id = u16::from_be_bytes([buf[2], buf[3]]);
        if protocol_id != 0 {
            return Err(ModbusTcpError::InvalidProtocolId);
        }
        let length = u16::from_be_bytes([buf[4], buf[5]]);
        let unit_id = buf[6];
        Ok(Self {
            transaction_id,
            protocol_id,
            length,
            unit_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_sets_protocol_id_zero() {
        let h = MbapHeader::new(0x1234, 0x05, 4);
        assert_eq!(h.transaction_id, 0x1234);
        assert_eq!(h.protocol_id, 0);
        assert_eq!(h.length, 5); // data_len(4) + 1
        assert_eq!(h.unit_id, 0x05);
    }

    #[test]
    fn test_new_data_len_zero() {
        // data_len=0 时 length=1（仅 unit_id）
        let h = MbapHeader::new(0, 1, 0);
        assert_eq!(h.length, 1);
    }

    #[test]
    fn test_encode_roundtrip() {
        let h = MbapHeader::new(0xABCD, 0x10, 8);
        let bytes = h.encode();
        assert_eq!(bytes.len(), 7);
        assert_eq!(bytes, [0xAB, 0xCD, 0x00, 0x00, 0x00, 0x09, 0x10]);
        let decoded = MbapHeader::decode(&bytes).expect("decode should succeed");
        assert_eq!(decoded, h);
    }

    #[test]
    fn test_decode_frame_too_short() {
        let buf = [0u8; 6];
        assert_eq!(MbapHeader::decode(&buf), Err(ModbusTcpError::FrameTooShort));
        // 空缓冲
        assert_eq!(MbapHeader::decode(&[]), Err(ModbusTcpError::FrameTooShort));
    }

    #[test]
    fn test_decode_invalid_protocol_id() {
        // protocol_id = 1 (非 Modbus)
        let buf = [0x00, 0x01, 0x00, 0x01, 0x00, 0x05, 0xFF];
        assert_eq!(
            MbapHeader::decode(&buf),
            Err(ModbusTcpError::InvalidProtocolId)
        );
    }

    #[test]
    fn test_decode_exactly_seven_bytes() {
        // 恰好 7 字节，合法
        let h = MbapHeader::new(0x0001, 0x02, 4);
        let bytes = h.encode();
        assert_eq!(bytes.len(), 7);
        let decoded = MbapHeader::decode(&bytes).expect("decode should succeed");
        assert_eq!(decoded.transaction_id, 0x0001);
        assert_eq!(decoded.unit_id, 0x02);
        assert_eq!(decoded.length, 5);
    }

    #[test]
    fn test_decode_extra_bytes_ignored() {
        // 缓冲超过 7 字节，前 7 字节解码
        let h = MbapHeader::new(0x4321, 0x07, 2);
        let mut bytes = h.encode().to_vec();
        bytes.extend_from_slice(&[0xAA, 0xBB]); // PDU 数据，decode 忽略
        let decoded = MbapHeader::decode(&bytes).expect("decode should succeed");
        assert_eq!(decoded, h);
    }

    #[test]
    fn test_encode_max_values() {
        let h = MbapHeader::new(0xFFFF, 0xFF, 0xFFFF - 1);
        let bytes = h.encode();
        assert_eq!(bytes, [0xFF, 0xFF, 0x00, 0x00, 0xFF, 0xFF, 0xFF]);
        let decoded = MbapHeader::decode(&bytes).expect("decode should succeed");
        assert_eq!(decoded, h);
    }
}
