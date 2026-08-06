//! MBAP（Modbus Application Protocol）头部编解码.
//!
//! MBAP 头部 7 字节，大端序：
//! - transaction_id (u16)：事务 ID，请求/响应配对
//! - protocol_id (u16)：协议 ID，Modbus = 0
//! - length (u16)：后续字节数（unit_id + PDU）
//! - unit_id (u8)：单元 ID（对应 RTU 的从站地址）

use alloc::vec::Vec;

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

/// MBAP 流式重组器（v0.218.0，B2-01）。
///
/// TCP 是字节流，无消息边界：多帧粘包或单帧半包都可能出现。
/// 本解码器按 MBAP `length` 字段重组：
/// - 缓冲不足 7 字节 → 继续累积（半包）；
/// - 缓冲 ≥ 7 字节 → 解析 length，等待完整 `7 + length` 字节；
/// - 完整后整帧出队，剩余字节继续解析（粘包）。
pub struct MbapStreamDecoder {
    /// 累积缓冲。
    buf: Vec<u8>,
    /// 重组完成的帧数。
    frames: u64,
    /// 丢弃的坏帧数（协议 ID 非法 / length 越界）。
    bad_frames: u64,
}

impl Default for MbapStreamDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl MbapStreamDecoder {
    /// 创建流式解码器。
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            frames: 0,
            bad_frames: 0,
        }
    }

    /// 喂入一段 TCP 字节流，返回本次重组出的完整帧（0..N 个）。
    ///
    /// MBAP `length` 字段 = unit_id(1) + PDU 长度；unit_id 已含在 7 字节
    /// 头部内，故完整帧总长 = 6 + length（= 7 头 + PDU）。
    pub fn feed(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        loop {
            // 需要至少 7 字节解析头部。
            if self.buf.len() < 7 {
                break;
            }
            let header = match MbapHeader::decode(&self.buf) {
                Ok(h) => h,
                Err(_) => {
                    // 协议 ID 非法：丢弃整段缓冲（帧边界不可信，重新同步），
                    // 只计 1 坏帧，避免噪声统计。
                    self.bad_frames += 1;
                    self.buf.clear();
                    break;
                }
            };
            let total = 6 + header.length as usize;
            if total > 4096 {
                // length 越界：丢弃头部，重新同步。
                self.bad_frames += 1;
                self.buf.drain(..7);
                continue;
            }
            if self.buf.len() < total {
                // 半包：等待更多数据。
                break;
            }
            let frame: Vec<u8> = self.buf.drain(..total).collect();
            self.frames += 1;
            out.push(frame);
        }
        out
    }

    /// 统计：重组帧数 / 坏帧数。
    pub fn stats(&self) -> (u64, u64) {
        (self.frames, self.bad_frames)
    }

    /// 清空缓冲（连接重置时）。
    pub fn reset(&mut self) {
        self.buf.clear();
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

    // ===== v0.218.0（B2-01）：流式重组（粘包/半包） =====

    /// 构造一帧（MBAP 头含 unit_id + PDU）。
    fn make_frame(txn: u16, unit: u8, pdu: &[u8]) -> Vec<u8> {
        let header = MbapHeader::new(txn, unit, pdu.len() as u16);
        let mut f = header.encode().to_vec();
        f.extend_from_slice(pdu);
        f
    }

    #[test]
    fn test_stream_single_frame() {
        let mut dec = MbapStreamDecoder::new();
        let frame = make_frame(0x0001, 0x01, &[0x03, 0x02, 0x00, 0x0A]);
        let out = dec.feed(&frame);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], frame);
        assert_eq!(dec.stats(), (1, 0));
    }

    #[test]
    fn test_stream_half_packet_reassembled() {
        let mut dec = MbapStreamDecoder::new();
        let frame = make_frame(0x0002, 0x01, &[0x03, 0x02, 0x00, 0x0A]);
        // 半包：先给 4 字节，再给剩余。
        assert!(dec.feed(&frame[..4]).is_empty());
        let out = dec.feed(&frame[4..]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], frame);
    }

    #[test]
    fn test_stream_multi_frame_glued() {
        let mut dec = MbapStreamDecoder::new();
        let f1 = make_frame(0x0001, 0x01, &[0x03, 0x02, 0x00, 0x0A]);
        let f2 = make_frame(0x0002, 0x01, &[0x06, 0x00, 0x01, 0x00, 0x02]);
        // 粘包：两帧一次给入。
        let mut glued = f1.clone();
        glued.extend_from_slice(&f2);
        let out = dec.feed(&glued);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], f1);
        assert_eq!(out[1], f2);
        assert_eq!(dec.stats(), (2, 0));
    }

    #[test]
    fn test_stream_partial_then_glued_remainder() {
        let mut dec = MbapStreamDecoder::new();
        let f1 = make_frame(0x0001, 0x01, &[0x03, 0x02, 0x00, 0x0A]);
        let f2 = make_frame(0x0002, 0x01, &[0x06, 0x00, 0x01, 0x00, 0x02]);
        // 第一帧完整 + 第二帧半包。
        let mut chunk = f1.clone();
        chunk.extend_from_slice(&f2[..5]);
        let out = dec.feed(&chunk);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], f1);
        // 第二帧剩余。
        let out = dec.feed(&f2[5..]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], f2);
        assert_eq!(dec.stats(), (2, 0));
    }

    #[test]
    fn test_stream_bad_protocol_id_resync() {
        let mut dec = MbapStreamDecoder::new();
        let frame = make_frame(0x0001, 0x01, &[0x03, 0x02, 0x00, 0x0A]);
        // 污染 protocol_id（第 3 字节 = 0x00 → 0x55）。
        let mut bad = frame.clone();
        bad[3] = 0x55;
        // 坏帧 + 好帧粘包：坏帧触发缓冲丢弃（fail-closed），好帧需重发。
        let mut glued = bad;
        glued.extend_from_slice(&frame);
        let out = dec.feed(&glued);
        assert!(out.is_empty(), "坏帧批次被丢弃");
        let (frames, bad_count) = dec.stats();
        assert_eq!(frames, 0);
        assert_eq!(bad_count, 1);
        // 重新同步后好帧可正常重组。
        let out = dec.feed(&frame);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], frame);
        assert_eq!(dec.stats(), (1, 1));
    }
}
