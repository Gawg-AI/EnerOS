//! IEC 104 APDU 帧结构（I/S/U 三种控制域格式）.
//!
//! 帧布局：StartByte(0x68) | Length(1) | ControlField(4) | [ASDU]
//! - I 格式：bit0=0，send_seq/recv_seq 各 15 位
//! - S 格式：bit0=1, bit1=1，recv_seq 15 位
//! - U 格式：bit0=1, bit1=0，功能码在 byte0

use alloc::vec::Vec;

use crate::asdu::Asdu;
use crate::error::Iec104Error;

/// U 格式功能（6 变体）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UFormatFunction {
    /// 启动数据传输激活
    StartDtAct,
    /// 启动数据传输确认
    StartDtCon,
    /// 停止数据传输激活
    StopDtAct,
    /// 停止数据传输确认
    StopDtCon,
    /// 测试帧激活
    TestFrAct,
    /// 测试帧确认
    TestFrCon,
}

impl UFormatFunction {
    /// 编码为控制域第 1 字节。
    fn to_ctrl_byte(self) -> u8 {
        match self {
            Self::StartDtAct => 0x07,
            Self::StartDtCon => 0x0B,
            Self::StopDtAct => 0x13,
            Self::StopDtCon => 0x17,
            Self::TestFrAct => 0x43,
            Self::TestFrCon => 0x83,
        }
    }

    /// 从控制域第 1 字节解码。
    fn from_ctrl_byte(b: u8) -> Option<Self> {
        match b {
            0x07 => Some(Self::StartDtAct),
            0x0B => Some(Self::StartDtCon),
            0x13 => Some(Self::StopDtAct),
            0x17 => Some(Self::StopDtCon),
            0x43 => Some(Self::TestFrAct),
            0x83 => Some(Self::TestFrCon),
            _ => None,
        }
    }
}

/// 控制域
#[derive(Debug, Clone, PartialEq)]
pub enum ControlField {
    /// I 格式（信息传输）
    Information {
        /// 发送序列号（15 位）
        send_seq: u16,
        /// 接收序列号（15 位）
        recv_seq: u16,
    },
    /// S 格式（确认）
    Numbered {
        /// 接收序列号（15 位）
        recv_seq: u16,
    },
    /// U 格式（控制功能）
    Unnumbered(UFormatFunction),
}

/// APDU（Application Protocol Data Unit）
#[derive(Debug, Clone, PartialEq)]
pub struct Apdu {
    /// 控制域
    pub control_field: ControlField,
    /// ASDU（U/S 格式为 `None`）
    pub asdu: Option<Asdu>,
}

impl Apdu {
    /// 创建 U 格式 APDU。
    pub fn u_format(func: UFormatFunction) -> Self {
        Self {
            control_field: ControlField::Unnumbered(func),
            asdu: None,
        }
    }

    /// 创建 S 格式 APDU。
    pub fn s_format(recv_seq: u16) -> Self {
        Self {
            control_field: ControlField::Numbered {
                recv_seq: recv_seq & 0x7FFF,
            },
            asdu: None,
        }
    }

    /// 创建 I 格式 APDU。
    pub fn i_format(send_seq: u16, recv_seq: u16, asdu: Asdu) -> Self {
        Self {
            control_field: ControlField::Information {
                send_seq: send_seq & 0x7FFF,
                recv_seq: recv_seq & 0x7FFF,
            },
            asdu: Some(asdu),
        }
    }

    /// 编码 APDU 为字节流。
    pub fn encode(&self) -> Vec<u8> {
        let mut ctrl = [0u8; 4];
        match &self.control_field {
            ControlField::Information { send_seq, recv_seq } => {
                // I 格式：bit0=0
                let s = (send_seq & 0x7FFF) << 1;
                let r = (recv_seq & 0x7FFF) << 1;
                ctrl[0] = (s & 0xFF) as u8;
                ctrl[1] = ((s >> 8) & 0xFF) as u8;
                ctrl[2] = (r & 0xFF) as u8;
                ctrl[3] = ((r >> 8) & 0xFF) as u8;
            }
            ControlField::Numbered { recv_seq } => {
                // S 格式：bit0=1, bit1=1
                ctrl[0] = 0x01;
                ctrl[1] = 0x00;
                let r = (recv_seq & 0x7FFF) << 1;
                ctrl[2] = (r & 0xFF) as u8;
                ctrl[3] = ((r >> 8) & 0xFF) as u8;
            }
            ControlField::Unnumbered(func) => {
                // U 格式：bit0=1, bit1=0
                ctrl[0] = func.to_ctrl_byte();
                ctrl[1] = 0x00;
                ctrl[2] = 0x00;
                ctrl[3] = 0x00;
            }
        }
        let asdu_bytes = self.asdu.as_ref().map(|a| a.encode()).unwrap_or_default();
        let total_len = 4 + asdu_bytes.len();
        let mut buf = Vec::with_capacity(2 + total_len);
        buf.push(0x68); // 起始字节
        buf.push(total_len as u8); // 长度
        buf.extend_from_slice(&ctrl);
        buf.extend_from_slice(&asdu_bytes);
        buf
    }

    /// 从字节流解码 APDU。
    pub fn decode(bytes: &[u8]) -> Result<Self, Iec104Error> {
        // 最小长度：start(1) + length(1) + ctrl(4) = 6
        if bytes.len() < 6 {
            return Err(Iec104Error::InvalidFrame);
        }
        if bytes[0] != 0x68 {
            return Err(Iec104Error::InvalidFrame);
        }
        let length = bytes[1] as usize;
        // 长度至少 4（控制域），且不超过剩余字节
        if length < 4 {
            return Err(Iec104Error::InvalidFrame);
        }
        if bytes.len() < 2 + length {
            return Err(Iec104Error::InvalidFrame);
        }
        let ctrl = &bytes[2..6];
        let control_field = if ctrl[0] & 0x01 == 0 {
            // I 格式：bit0=0
            let send_seq = (((ctrl[0] as u16) | ((ctrl[1] as u16) << 8)) >> 1) & 0x7FFF;
            let recv_seq = (((ctrl[2] as u16) | ((ctrl[3] as u16) << 8)) >> 1) & 0x7FFF;
            ControlField::Information { send_seq, recv_seq }
        } else if ctrl[0] == 0x01 {
            // S 格式：bit0=1, bit1=0（控制域字节 1 恰为 0x01）
            let recv_seq = (((ctrl[2] as u16) | ((ctrl[3] as u16) << 8)) >> 1) & 0x7FFF;
            ControlField::Numbered { recv_seq }
        } else {
            // U 格式：bit0=1, bit1=1（功能码在 byte0，由 from_ctrl_byte 精确匹配）
            let func = UFormatFunction::from_ctrl_byte(ctrl[0]).ok_or(Iec104Error::InvalidFrame)?;
            ControlField::Unnumbered(func)
        };
        // 解析 ASDU（如果长度 > 4）
        let asdu = if length > 4 {
            let asdu_bytes = &bytes[6..2 + length];
            Some(Asdu::decode(asdu_bytes)?)
        } else {
            None
        };
        Ok(Self {
            control_field,
            asdu,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asdu::{Cot, InformationObject, IoValue, QualityDescriptor, TypeId};

    fn make_simple_asdu() -> Asdu {
        Asdu {
            type_id: TypeId::SinglePointInformation,
            cause_of_tx: Cot::Periodic,
            common_addr: 1,
            ioas: vec![InformationObject {
                ioa: 1,
                value: IoValue::SinglePoint(crate::asdu::SinglePointValue::On),
                quality: QualityDescriptor::good(),
                time_tag: None,
            }],
        }
    }

    // ===== U 格式测试 =====

    #[test]
    fn test_u_format_startdt_act() {
        let apdu = Apdu::u_format(UFormatFunction::StartDtAct);
        let bytes = apdu.encode();
        assert_eq!(bytes, [0x68, 0x04, 0x07, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_u_format_startdt_con() {
        let apdu = Apdu::u_format(UFormatFunction::StartDtCon);
        let bytes = apdu.encode();
        assert_eq!(bytes, [0x68, 0x04, 0x0B, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_u_format_stopdt_act() {
        let apdu = Apdu::u_format(UFormatFunction::StopDtAct);
        let bytes = apdu.encode();
        assert_eq!(bytes, [0x68, 0x04, 0x13, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_u_format_stopdt_con() {
        let apdu = Apdu::u_format(UFormatFunction::StopDtCon);
        let bytes = apdu.encode();
        assert_eq!(bytes, [0x68, 0x04, 0x17, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_u_format_testfr_act() {
        let apdu = Apdu::u_format(UFormatFunction::TestFrAct);
        let bytes = apdu.encode();
        assert_eq!(bytes, [0x68, 0x04, 0x43, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_u_format_testfr_con() {
        let apdu = Apdu::u_format(UFormatFunction::TestFrCon);
        let bytes = apdu.encode();
        assert_eq!(bytes, [0x68, 0x04, 0x83, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_u_format_all_roundtrip() {
        let funcs = [
            UFormatFunction::StartDtAct,
            UFormatFunction::StartDtCon,
            UFormatFunction::StopDtAct,
            UFormatFunction::StopDtCon,
            UFormatFunction::TestFrAct,
            UFormatFunction::TestFrCon,
        ];
        for func in &funcs {
            let apdu = Apdu::u_format(*func);
            let bytes = apdu.encode();
            let decoded = Apdu::decode(&bytes).expect("decode ok");
            assert_eq!(decoded, apdu);
        }
    }

    // ===== S 格式测试 =====

    #[test]
    fn test_s_format_encode() {
        let apdu = Apdu::s_format(0);
        let bytes = apdu.encode();
        assert_eq!(bytes, [0x68, 0x04, 0x01, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_s_format_decode() {
        // spec scenario: [0x68, 0x04, 0x01, 0x00, 0x02, 0x00] → recv_seq=1
        let bytes = [0x68, 0x04, 0x01, 0x00, 0x02, 0x00];
        let decoded = Apdu::decode(&bytes).expect("decode ok");
        assert!(matches!(
            &decoded.control_field,
            ControlField::Numbered { recv_seq } if *recv_seq == 1
        ));
    }

    #[test]
    fn test_s_format_roundtrip_various() {
        for recv_seq in [0u16, 1, 100, 32767] {
            let apdu = Apdu::s_format(recv_seq);
            let bytes = apdu.encode();
            let decoded = Apdu::decode(&bytes).expect("decode ok");
            assert_eq!(decoded, apdu);
        }
    }

    // ===== I 格式测试 =====

    #[test]
    fn test_i_format_encode_bit0_zero() {
        let asdu = make_simple_asdu();
        let apdu = Apdu::i_format(0, 0, asdu);
        let bytes = apdu.encode();
        assert_eq!(bytes[0], 0x68);
        assert_eq!(bytes[2] & 0x01, 0); // I 格式 bit0=0
    }

    #[test]
    fn test_i_format_roundtrip() {
        let asdu = make_simple_asdu();
        let apdu = Apdu::i_format(5, 3, asdu);
        let bytes = apdu.encode();
        let decoded = Apdu::decode(&bytes).expect("decode ok");
        assert!(matches!(
            &decoded.control_field,
            ControlField::Information { send_seq, recv_seq } if *send_seq == 5 && *recv_seq == 3
        ));
        assert_eq!(decoded.asdu, apdu.asdu);
    }

    #[test]
    fn test_i_format_seq_zero() {
        let asdu = make_simple_asdu();
        let apdu = Apdu::i_format(0, 0, asdu);
        let bytes = apdu.encode();
        assert_eq!(bytes[2], 0x00); // send_seq=0 → 0<<1=0
        assert_eq!(bytes[3], 0x00);
        assert_eq!(bytes[4], 0x00); // recv_seq=0
        assert_eq!(bytes[5], 0x00);
    }

    // ===== 15 位序列号回绕 =====

    #[test]
    fn test_sequence_wraparound_32767() {
        let asdu = make_simple_asdu();
        // send_seq=32767 → 32767<<1 = 65534 = 0xFFFE
        let apdu = Apdu::i_format(32767, 0, asdu);
        let bytes = apdu.encode();
        assert_eq!(bytes[2], 0xFE);
        assert_eq!(bytes[3], 0xFF);
        let decoded = Apdu::decode(&bytes).expect("decode ok");
        assert!(matches!(
            &decoded.control_field,
            ControlField::Information { send_seq, .. } if *send_seq == 32767
        ));
    }

    #[test]
    fn test_sequence_wraparound_to_zero() {
        // 32768 & 0x7FFF = 0
        let asdu = make_simple_asdu();
        let apdu = Apdu::i_format(32768, 0, asdu);
        assert!(matches!(
            &apdu.control_field,
            ControlField::Information { send_seq, .. } if *send_seq == 0
        ));
    }

    // ===== 边界测试 =====

    #[test]
    fn test_decode_empty() {
        assert_eq!(Apdu::decode(&[]), Err(Iec104Error::InvalidFrame));
    }

    #[test]
    fn test_decode_too_short() {
        assert_eq!(
            Apdu::decode(&[0x68, 0x04, 0x01]),
            Err(Iec104Error::InvalidFrame)
        );
    }

    #[test]
    fn test_decode_wrong_start_byte() {
        assert_eq!(
            Apdu::decode(&[0x00, 0x04, 0x01, 0x00, 0x00, 0x00]),
            Err(Iec104Error::InvalidFrame)
        );
    }

    #[test]
    fn test_decode_length_mismatch() {
        // 声明长度 10 但实际只有 6 字节
        assert_eq!(
            Apdu::decode(&[0x68, 0x0A, 0x01, 0x00, 0x00, 0x00]),
            Err(Iec104Error::InvalidFrame)
        );
    }

    #[test]
    fn test_decode_invalid_u_format_byte() {
        // ctrl[0]=0x05: bit0=1, bit1=0 → U 格式，但 0x05 不是合法 U 功能码
        assert_eq!(
            Apdu::decode(&[0x68, 0x04, 0x05, 0x00, 0x00, 0x00]),
            Err(Iec104Error::InvalidFrame)
        );
    }
}
