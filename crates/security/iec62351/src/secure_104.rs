//! IEC 104 over TLS 安全层（v0.237.0，B4-04；26-B WP-26.5 诚实命名）。
//!
//! # 诚实声明（首行）
//! **本模块的 [`FramingRecordCodec`]（原 `FramingRecordCodec`）仅做记录分帧
//! （`type(1) ‖ len(2 BE) ‖ payload`），不含任何加密。** 真 IEC 62351-3 TLS
//! 为独立未实现能力（依赖 `eneros-tls` 的 target 支持，蓝图/19 P0-14）；
//! `PlainRecordCodec` 明文直通是显式降级（B4-04 遗留，非安全声明）。
//!
//! - [`Secure104Session`] — 会话抽象：握手 → 记录保护（加密）→ 收发；
//! - [`FramingRecordCodec`] — 记录层**分帧**编解码（长度前缀 + 类型标记）；
//! - feature `tls`：std 侧 rustls 实现（对接 `eneros-tls`
//!   `TlsClientConfig`/`TlsServerConfig`）。

use alloc::vec::Vec;

use crate::SecError;

/// TLS 记录类型（IEC 62351-3 语义，非 RFC 编号）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordType {
    /// 握手记录。
    Handshake = 1,
    /// 应用数据记录。
    ApplicationData = 2,
    /// 告警（安全事件）。
    Alert = 3,
}

/// 记录层编解码抽象。
pub trait RecordCodec {
    /// 编码记录：`type(1) ‖ len(2 BE) ‖ payload`。
    fn encode(&self, rtype: RecordType, payload: &[u8]) -> Vec<u8>;
    /// 解码记录：返回 (类型, payload)；非法格式 → Err。
    fn decode(&self, bytes: &[u8]) -> Result<(RecordType, Vec<u8>), SecError>;
}

/// 记录**分帧**编解码（WP-26.5：原 `FramingRecordCodec` 更名——本 codec 仅做
/// 记录分帧（`type(1) ‖ len(2 BE) ‖ payload`），**不含任何加密**；真
/// 62351-3 TLS 为独立未实现能力，见模块文档首行）。
pub struct FramingRecordCodec;

impl RecordCodec for FramingRecordCodec {
    fn encode(&self, rtype: RecordType, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(3 + payload.len());
        out.push(rtype as u8);
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    fn decode(&self, bytes: &[u8]) -> Result<(RecordType, Vec<u8>), SecError> {
        if bytes.len() < 3 {
            return Err(SecError::FrameInvalid);
        }
        let rtype = match bytes[0] {
            1 => RecordType::Handshake,
            2 => RecordType::ApplicationData,
            3 => RecordType::Alert,
            _ => return Err(SecError::FrameInvalid),
        };
        let len = u16::from_be_bytes([bytes[1], bytes[2]]) as usize;
        if bytes.len() < 3 + len {
            return Err(SecError::FrameInvalid);
        }
        Ok((rtype, bytes[3..3 + len].to_vec()))
    }
}

/// 明文直通（无 TLS 编译目标时的显式降级）。
pub struct PlainRecordCodec;

impl RecordCodec for PlainRecordCodec {
    fn encode(&self, rtype: RecordType, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(3 + payload.len());
        out.push(rtype as u8);
        out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    fn decode(&self, bytes: &[u8]) -> Result<(RecordType, Vec<u8>), SecError> {
        if bytes.len() < 3 {
            return Err(SecError::FrameInvalid);
        }
        let len = u16::from_be_bytes([bytes[1], bytes[2]]) as usize;
        if bytes.len() < 3 + len {
            return Err(SecError::FrameInvalid);
        }
        let rtype = match bytes[0] {
            1 => RecordType::Handshake,
            2 => RecordType::ApplicationData,
            _ => return Err(SecError::FrameInvalid),
        };
        Ok((rtype, bytes[3..3 + len].to_vec()))
    }
}

/// IEC104 over TLS 会话。
///
/// 组合记录编解码 + 会话状态（握手完成才可传输应用数据）。
pub struct Secure104Session<C: RecordCodec> {
    /// 记录编解码。
    codec: C,
    /// 握手是否完成。
    /// 会话就绪标记（34 SE-14 更名：无密码学握手）。
    session_ready: bool,
    /// 已收/已发记录计数。
    sent: u64,
    recvd: u64,
}

impl<C: RecordCodec> Secure104Session<C> {
    /// 创建安全会话。
    pub fn new(codec: C) -> Self {
        Self {
            codec,
            session_ready: false,
            sent: 0,
            recvd: 0,
        }
    }

    /// 标记会话就绪（34 SE-14 更名：旧 `handshake` 是空壳——只编码
    /// 记录、无任何密码学握手；**本方法不进行安全握手**，仅由集成层在
    /// 真实 TLS 握手（rustls 等）完成后调用。真握手立项见 SE-10 协同。
    ///
    /// `hello` 为握手载荷（std TLS 路径由 rustls 生成；测试注入）。
    pub fn mark_session_ready(&mut self, hello: &[u8]) -> Result<(), SecError> {
        let rec = self.codec.encode(RecordType::Handshake, hello);
        // 真实路径：发送 rec 并等待对端握手完成（此处由集成层闭环）。
        let _ = rec;
        self.session_ready = true;
        Ok(())
    }

    /// 会话是否就绪（未就绪时 `send_application` 拒绝）。
    pub fn session_ready(&self) -> bool {
        self.session_ready
    }

    /// 发送应用记录（IEC104 APDU 封装）。
    pub fn send_application(&mut self, apdu: &[u8]) -> Result<Vec<u8>, SecError> {
        if !self.session_ready {
            return Err(SecError::NotReady);
        }
        let rec = self.codec.encode(RecordType::ApplicationData, apdu);
        self.sent += 1;
        Ok(rec)
    }

    /// 接收并解析记录（返回应用数据；握手/告警记录分离处理）。
    pub fn receive(&mut self, bytes: &[u8]) -> Result<Option<Vec<u8>>, SecError> {
        let (rtype, payload) = self.codec.decode(bytes)?;
        self.recvd += 1;
        match rtype {
            RecordType::ApplicationData => Ok(Some(payload)),
            RecordType::Alert => Err(SecError::PeerAlert),
            RecordType::Handshake => Ok(None),
        }
    }

    /// 统计。
    pub fn stats(&self) -> (u64, u64) {
        (self.sent, self.recvd)
    }
}

// ============================================================================
// Tests（记录编解码 + 会话状态机）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tls_record_roundtrip() {
        let codec = FramingRecordCodec;
        let rec = codec.encode(RecordType::ApplicationData, &[0x68, 0x0A, 1, 2, 3]);
        let (t, payload) = codec.decode(&rec).expect("decode");
        assert_eq!(t, RecordType::ApplicationData);
        assert_eq!(payload, vec![0x68, 0x0A, 1, 2, 3]);
    }

    #[test]
    fn test_record_truncated_rejected() {
        let codec = FramingRecordCodec;
        assert_eq!(codec.decode(&[2, 0, 5, 1]), Err(SecError::FrameInvalid));
        assert_eq!(codec.decode(&[9, 0, 0]), Err(SecError::FrameInvalid));
    }

    #[test]
    fn test_handshake_record_type() {
        let codec = FramingRecordCodec;
        let rec = codec.encode(RecordType::Handshake, b"clienthello");
        let (t, p) = codec.decode(&rec).expect("decode");
        assert_eq!(t, RecordType::Handshake);
        assert_eq!(p, b"clienthello");
    }

    #[test]
    fn test_session_requires_handshake_before_send() {
        let mut session = Secure104Session::new(FramingRecordCodec);
        // 未握手 → 拒绝应用发送。
        assert_eq!(session.send_application(b"apdu"), Err(SecError::NotReady));
        // 握手后允许。
        session.mark_session_ready(b"hello").expect("ready");
        assert!(session.session_ready());
        let rec = session.send_application(b"apdu").expect("send");
        assert_eq!(rec[0], 2); // ApplicationData
        assert_eq!(rec.len(), 3 + 4);
    }

    #[test]
    fn test_session_receive_application_data() {
        let mut session = Secure104Session::new(FramingRecordCodec);
        session.mark_session_ready(b"h").expect("ready");
        let rec = session.send_application(&[0x68, 0x0A]).expect("send");
        let data = session.receive(&rec).expect("recv").expect("app data");
        assert_eq!(data, vec![0x68, 0x0A]);
    }

    #[test]
    fn test_session_alert_rejected() {
        let codec = FramingRecordCodec;
        let mut session = Secure104Session::new(FramingRecordCodec);
        session.mark_session_ready(b"h").expect("ready");
        let alert = codec.encode(RecordType::Alert, b"peer-error");
        assert_eq!(session.receive(&alert), Err(SecError::PeerAlert));
    }

    #[test]
    fn test_plain_codec_degrades() {
        // 明文直通：与 TLS 记录格式一致（长度前缀），显式降级档。
        let codec = PlainRecordCodec;
        let rec = codec.encode(RecordType::ApplicationData, b"data");
        let (t, p) = codec.decode(&rec).expect("decode");
        assert_eq!(t, RecordType::ApplicationData);
        assert_eq!(p, b"data");
    }

    // ===== WP-26.5：无加密事实固定——FramingRecordCodec 仅分帧不加密 =====
    // 把"当前无加密"变成可回归的事实：payload 明文原样出现在输出中
    // （偏移 3 起逐字节相等）。未来接入真 62351-3 TLS 时本测试必须被
    // 显式重写，并在提交说明中引用本 WP（26-B §6 第 3 条）。
    #[test]
    fn test_wp265_framing_codec_plaintext_fact() {
        let codec = FramingRecordCodec;
        let payload = b"P_total=123.45;Q=0.0;timestamp=1700000000";
        let encoded = codec.encode(RecordType::ApplicationData, payload);
        // 帧格式：type(1) ‖ len(2 BE) ‖ payload——payload 明文原样出现。
        assert_eq!(encoded.len(), 3 + payload.len());
        assert_eq!(
            &encoded[3..],
            payload,
            "payload 必须明文原样出现（无加密事实）"
        );
        // 解码还原明文。
        let (rtype, decoded) = codec.decode(&encoded).expect("decode");
        assert_eq!(rtype, RecordType::ApplicationData);
        assert_eq!(&decoded, payload);
    }
}
