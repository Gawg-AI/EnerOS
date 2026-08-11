//! PPP (Point-to-Point Protocol) dial-up negotiation (workspace v0.192.0).
//!
//! Provides a minimal PPP state machine and HDLC frame encoding/decoding
//! for cellular modem dial-up. Full PPP stack (complete LCP/IPCP/PAP/CHAP
//! packets, CRC-16-CCITT FCS) requires hardware verification.

use alloc::vec::Vec;

use crate::error::CellularError;

/// IPv4 address type (re-export from smoltcp).
pub type Ipv4Addr = smoltcp::wire::Ipv4Address;

/// HDLC Flag sequence.
pub const HDLC_FLAG: u8 = 0x7E;
/// HDLC Escape character.
pub const HDLC_ESCAPE: u8 = 0x7D;
/// PPP protocol: LCP (Link Control Protocol).
pub const PPP_LCP: u16 = 0xC021;
/// PPP protocol: IPCP (IP Control Protocol).
pub const PPP_IPCP: u16 = 0x8021;
/// PPP protocol: PAP (Password Authentication Protocol).
pub const PPP_PAP: u16 = 0xC023;
/// PPP protocol: CHAP (Challenge-Handshake Auth Protocol).
pub const PPP_CHAP: u16 = 0xC223;
/// PPP protocol: IP (Internet Protocol).
pub const PPP_IP: u16 = 0x0021;

/// PPP state machine states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PppState {
    /// Link is closed.
    Closed,
    /// LCP negotiation in progress.
    Establishing,
    /// PAP/CHAP authentication in progress.
    Authenticating,
    /// IPCP negotiation in progress.
    Networking,
    /// IP obtained, link is up.
    Connected,
    /// Link is being torn down.
    Terminating,
}

/// A PPP frame (HDLC encapsulated).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PppFrame {
    /// PPP protocol identifier (e.g. `PPP_LCP`, `PPP_IP`).
    pub protocol: u16,
    /// Frame payload (unescaped).
    pub data: Vec<u8>,
}

/// PPP state machine for dial-up negotiation.
pub struct PppStateMachine {
    state: PppState,
    retry_count: u32,
    max_retries: u32,
    assigned_ip: Option<Ipv4Addr>,
}

impl PppStateMachine {
    /// Create a new PPP state machine with the given retry budget.
    pub fn new(max_retries: u32) -> Self {
        Self {
            state: PppState::Closed,
            retry_count: 0,
            max_retries,
            assigned_ip: None,
        }
    }

    /// Current state of the negotiation.
    pub fn state(&self) -> PppState {
        self.state
    }

    /// Begin negotiation: `Closed` -> `Establishing`.
    ///
    /// Returns `Err(PppNegotiationFailed)` if not currently `Closed`.
    pub fn start(&mut self) -> Result<(), CellularError> {
        if self.state != PppState::Closed {
            return Err(CellularError::PppNegotiationFailed);
        }
        self.state = PppState::Establishing;
        Ok(())
    }

    /// LCP Config-Ack received: `Establishing` -> `Authenticating`.
    pub fn on_lcp_config_ack(&mut self) -> Result<(), CellularError> {
        if self.state != PppState::Establishing {
            return Err(CellularError::PppNegotiationFailed);
        }
        self.state = PppState::Authenticating;
        Ok(())
    }

    /// Authentication succeeded: `Authenticating` -> `Networking`.
    pub fn on_auth_success(&mut self) -> Result<(), CellularError> {
        if self.state != PppState::Authenticating {
            return Err(CellularError::PppNegotiationFailed);
        }
        self.state = PppState::Networking;
        Ok(())
    }

    /// IPCP Config-Ack received with assigned IP: `Networking` -> `Connected`.
    pub fn on_ipcp_config_ack(&mut self, ip: Ipv4Addr) -> Result<(), CellularError> {
        if self.state != PppState::Networking {
            return Err(CellularError::PppNegotiationFailed);
        }
        self.assigned_ip = Some(ip);
        self.state = PppState::Connected;
        Ok(())
    }

    /// Handle a negotiation error. Increments `retry_count`.
    ///
    /// Returns `Ok(())` while retries remain, or `Err(PppNegotiationFailed)`
    /// and transitions to `Terminating` once the retry budget is exhausted.
    pub fn on_error(&mut self) -> Result<(), CellularError> {
        self.retry_count += 1;
        if self.retry_count < self.max_retries {
            Ok(())
        } else {
            self.state = PppState::Terminating;
            Err(CellularError::PppNegotiationFailed)
        }
    }

    /// Terminate the link: transitions through `Terminating` then `Closed`,
    /// and clears any assigned IP.
    pub fn terminate(&mut self) {
        self.state = PppState::Terminating;
        self.state = PppState::Closed;
        self.assigned_ip = None;
    }

    /// IP address assigned by the peer during IPCP, if any.
    pub fn assigned_ip(&self) -> Option<Ipv4Addr> {
        self.assigned_ip
    }

    /// Number of retries attempted so far.
    pub fn retry_count(&self) -> u32 {
        self.retry_count
    }
}

impl PppFrame {
    /// Create a new PPP frame with the given protocol and payload.
    pub fn new(protocol: u16, data: Vec<u8>) -> Self {
        Self { protocol, data }
    }

    /// Encode this frame to HDLC-encoded bytes per RFC 1662.
    ///
    /// 布局：`Flag | escape(Protocol(2,BE) + Data + FCS(2,LE)) | Flag`。
    /// 31-review（B3）：FCS 算**原始** protocol+data（非转义后），转义作用于
    /// content+FCS 整体——旧实现算转义后数据且 FCS 字节不转义，含 0x7E/0x7D
    /// 的帧被对端误读为假 flag/假转义。
    pub fn encode(&self) -> Vec<u8> {
        // 原始内容（未转义）：protocol + data。
        let mut content = Vec::with_capacity(2 + self.data.len());
        content.push((self.protocol >> 8) as u8);
        content.push((self.protocol & 0xFF) as u8);
        content.extend_from_slice(&self.data);
        // FCS 覆盖原始内容（RFC 1662 §6.2）。
        let fcs = fcs16(&content);
        let mut out = Vec::new();
        out.push(HDLC_FLAG);
        for &b in &content {
            push_escaped(&mut out, b);
        }
        // FCS 字节同样转义（小端，与 decode 的 LE 读取一致）。
        push_escaped(&mut out, (fcs & 0xFF) as u8);
        push_escaped(&mut out, (fcs >> 8) as u8);
        out.push(HDLC_FLAG);
        out
    }

    /// Decode an HDLC-encoded frame per RFC 1662.
    ///
    /// 31-review（B3）：先 unescape 整个中间字段（content+FCS），再按原始字节
    /// 实校验 FCS（全帧余数须 0xF0B8）——旧实现读转义后字节 + BE 读 FCS + body
    /// 为 `let _ = 0;` 空操作，坏帧静默接受。
    pub fn decode(raw: &[u8]) -> Result<PppFrame, CellularError> {
        // Minimum: Flag + Protocol(2) + FCS(2) + Flag = 6 bytes.
        if raw.len() < 6 {
            return Err(CellularError::DialFailed);
        }
        if raw.first() != Some(&HDLC_FLAG) || raw.last() != Some(&HDLC_FLAG) {
            return Err(CellularError::PppNegotiationFailed);
        }
        let middle = &raw[1..raw.len() - 1]; // 转义后的 content+FCS（去首尾 flag）
        if middle.len() < 4 {
            return Err(CellularError::DialFailed);
        }
        // 先 unescape 整个 middle（content+FCS）。
        let decoded = unescape(middle)?;
        if decoded.len() < 4 {
            return Err(CellularError::PppNegotiationFailed);
        }
        let fcs_off = decoded.len() - 2;
        let content = &decoded[..fcs_off];
        // 31-review（B3）：实校验——重算 content 的 FCS 与收到的 FCS（LE）比对。
        // encode 存 fcs16(content) 的 LE；此处对称重算（不依赖 0xF0B8 余数恒等式，
        // 该恒等式要求 FCS 经特定补码变换——本实现 fcs16 已返回补码，直接比对更稳）。
        let received_fcs = u16::from_le_bytes([decoded[fcs_off], decoded[fcs_off + 1]]);
        if fcs16(content) != received_fcs {
            return Err(CellularError::PppNegotiationFailed);
        }
        let protocol = ((decoded[0] as u16) << 8) | (decoded[1] as u16);
        let data = content[2..].to_vec();
        Ok(PppFrame { protocol, data })
    }
}

/// 31-review（B3）：HDLC transparency——单字节写入 out（0x7E→7D 5E，0x7D→7D 5D）。
fn push_escaped(out: &mut Vec<u8>, b: u8) {
    match b {
        HDLC_FLAG => {
            out.push(HDLC_ESCAPE);
            out.push(0x5E);
        }
        HDLC_ESCAPE => {
            out.push(HDLC_ESCAPE);
            out.push(0x5D);
        }
        _ => out.push(b),
    }
}

/// 31-review（B3）：反转义 HDLC 字节流（0x7D 5E→7E，0x7D 5D→7D）；非法转义序列 → Err。
fn unescape(esc: &[u8]) -> Result<Vec<u8>, CellularError> {
    let mut out = Vec::with_capacity(esc.len());
    let mut i = 0;
    while i < esc.len() {
        let b = esc[i];
        if b == HDLC_ESCAPE {
            if i + 1 >= esc.len() {
                return Err(CellularError::PppNegotiationFailed);
            }
            match esc[i + 1] {
                0x5E => out.push(HDLC_FLAG),
                0x5D => out.push(HDLC_ESCAPE),
                _ => return Err(CellularError::PppNegotiationFailed),
            }
            i += 2;
        } else {
            out.push(b);
            i += 1;
        }
    }
    Ok(out)
}

/// 31 DR-05：PPP FCS16（CRC-16-CCITT，RFC 1662 §6.2——多项式 0x8408
/// 反射，初始 0xFFFF，返回**补码**（传输语义；验证向量 "123456789" →
/// 0x906E；全帧（含 FCS）余数 = 0xF0B8）。
pub fn fcs16(data: &[u8]) -> u16 {
    let mut fcs: u16 = 0xFFFF;
    for &b in data {
        fcs ^= u16::from(b);
        for _ in 0..8 {
            fcs = if fcs & 0x0001 != 0 {
                (fcs >> 1) ^ 0x8408
            } else {
                fcs >> 1
            };
        }
    }
    0xFFFF ^ fcs
}

// ============================================================================
// PPP 报文解析（v0.197.0 P0-5：真实协商，IP 取自 IPCP 而非硬编码）
// ============================================================================

/// LCP 报文类型。
pub mod lcp {
    /// Configure-Request。
    pub const CONFIGURE_REQUEST: u8 = 0x01;
    /// Configure-Ack。
    pub const CONFIGURE_ACK: u8 = 0x02;
    /// Configure-Nak。
    pub const CONFIGURE_NAK: u8 = 0x03;
    /// Terminate-Request。
    pub const TERMINATE_REQUEST: u8 = 0x05;
    /// Terminate-Ack。
    pub const TERMINATE_ACK: u8 = 0x06;
}

/// IPCP 报文类型。
pub mod ipcp {
    /// Configure-Request。
    pub const CONFIGURE_REQUEST: u8 = 0x01;
    /// Configure-Ack。
    pub const CONFIGURE_ACK: u8 = 0x02;
    /// Configure-Nak。
    pub const CONFIGURE_NAK: u8 = 0x03;
    /// IP-Address 选项类型（RFC 1332 §3.2）。
    pub const OPT_IP_ADDRESS: u8 = 0x03;
}

/// 解析 IPCP Configure-* 报文中的 IP 地址。
///
/// IPCP 选项 `IP-Address`（类型 3，长度 6：type+len+4B IP）携带本端/对端
/// IP。返回 `Some(ip)` 若找到，否则 `None`。
pub fn parse_ipcp_ip(frame: &PppFrame) -> Option<Ipv4Addr> {
    if frame.protocol != PPP_IPCP {
        return None;
    }
    // data: [code, id, len, options...]
    let data = &frame.data;
    if data.len() < 4 {
        return None;
    }
    let code = data[0];
    if !matches!(
        code,
        ipcp::CONFIGURE_REQUEST | ipcp::CONFIGURE_ACK | ipcp::CONFIGURE_NAK
    ) {
        return None;
    }
    // options 从 data[4..] 开始（code+id+len = 4 字节头）。
    let mut i = 4;
    while i + 1 < data.len() {
        let opt_type = data[i];
        let opt_len = data[i + 1] as usize;
        if opt_len < 2 || i + opt_len > data.len() {
            break;
        }
        if opt_type == ipcp::OPT_IP_ADDRESS && opt_len == 6 {
            let ip = Ipv4Addr::new(data[i + 2], data[i + 3], data[i + 4], data[i + 5]);
            return Some(ip);
        }
        i += opt_len;
    }
    None
}

/// 解析 LCP 报文类型（返回 `Option<u8>` code）。
pub fn parse_lcp_code(frame: &PppFrame) -> Option<u8> {
    if frame.protocol != PPP_LCP {
        return None;
    }
    frame.data.first().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(a: u8, b: u8, c: u8, d: u8) -> Ipv4Addr {
        Ipv4Addr::new(a, b, c, d)
    }

    // --- State machine tests ---

    #[test]
    fn new_state_is_closed() {
        let m = PppStateMachine::new(3);
        assert_eq!(m.state(), PppState::Closed);
        assert_eq!(m.retry_count(), 0);
        assert_eq!(m.assigned_ip(), None);
    }

    #[test]
    fn start_transitions_to_establishing() {
        let mut m = PppStateMachine::new(3);
        assert!(m.start().is_ok());
        assert_eq!(m.state(), PppState::Establishing);
    }

    #[test]
    fn start_fails_when_not_closed() {
        let mut m = PppStateMachine::new(3);
        assert!(m.start().is_ok());
        assert_eq!(m.start().unwrap_err(), CellularError::PppNegotiationFailed);
        assert_eq!(m.state(), PppState::Establishing);
    }

    #[test]
    fn lcp_ack_transitions_to_authenticating() {
        let mut m = PppStateMachine::new(3);
        assert!(m.start().is_ok());
        assert!(m.on_lcp_config_ack().is_ok());
        assert_eq!(m.state(), PppState::Authenticating);
    }

    #[test]
    fn lcp_ack_fails_when_not_establishing() {
        let mut m = PppStateMachine::new(3);
        // From Closed state.
        assert_eq!(
            m.on_lcp_config_ack().unwrap_err(),
            CellularError::PppNegotiationFailed
        );
        assert_eq!(m.state(), PppState::Closed);
    }

    #[test]
    fn auth_success_transitions_to_networking() {
        let mut m = PppStateMachine::new(3);
        m.start().unwrap();
        m.on_lcp_config_ack().unwrap();
        assert!(m.on_auth_success().is_ok());
        assert_eq!(m.state(), PppState::Networking);
    }

    #[test]
    fn auth_success_fails_when_not_authenticating() {
        let mut m = PppStateMachine::new(3);
        m.start().unwrap();
        // Still Establishing.
        assert_eq!(
            m.on_auth_success().unwrap_err(),
            CellularError::PppNegotiationFailed
        );
        assert_eq!(m.state(), PppState::Establishing);
    }

    #[test]
    fn ipcp_ack_transitions_to_connected_and_sets_ip() {
        let mut m = PppStateMachine::new(3);
        m.start().unwrap();
        m.on_lcp_config_ack().unwrap();
        m.on_auth_success().unwrap();
        let assigned = ip(10, 0, 1, 5);
        assert!(m.on_ipcp_config_ack(assigned).is_ok());
        assert_eq!(m.state(), PppState::Connected);
        assert_eq!(m.assigned_ip(), Some(assigned));
    }

    #[test]
    fn ipcp_ack_fails_when_not_networking() {
        let mut m = PppStateMachine::new(3);
        m.start().unwrap();
        m.on_lcp_config_ack().unwrap();
        // Authenticating, not Networking.
        assert_eq!(
            m.on_ipcp_config_ack(ip(1, 2, 3, 4)).unwrap_err(),
            CellularError::PppNegotiationFailed
        );
        assert_eq!(m.assigned_ip(), None);
    }

    #[test]
    fn on_error_succeeds_while_retries_remain() {
        let mut m = PppStateMachine::new(3);
        assert!(m.on_error().is_ok());
        assert_eq!(m.retry_count(), 1);
        assert_eq!(m.state(), PppState::Closed);
        assert!(m.on_error().is_ok());
        assert_eq!(m.retry_count(), 2);
        assert_eq!(m.state(), PppState::Closed);
    }

    #[test]
    fn on_error_terminates_when_exhausted() {
        let mut m = PppStateMachine::new(2);
        assert!(m.on_error().is_ok());
        assert_eq!(m.retry_count(), 1);
        assert_eq!(
            m.on_error().unwrap_err(),
            CellularError::PppNegotiationFailed
        );
        assert_eq!(m.retry_count(), 2);
        assert_eq!(m.state(), PppState::Terminating);
    }

    #[test]
    fn terminate_resets_to_closed() {
        let mut m = PppStateMachine::new(3);
        m.start().unwrap();
        m.on_lcp_config_ack().unwrap();
        m.on_auth_success().unwrap();
        m.on_ipcp_config_ack(ip(192, 168, 1, 1)).unwrap();
        assert_eq!(m.state(), PppState::Connected);
        m.terminate();
        assert_eq!(m.state(), PppState::Closed);
        assert_eq!(m.assigned_ip(), None);
    }

    #[test]
    fn assigned_ip_none_before_ipcp() {
        let mut m = PppStateMachine::new(3);
        m.start().unwrap();
        m.on_lcp_config_ack().unwrap();
        m.on_auth_success().unwrap();
        assert_eq!(m.assigned_ip(), None);
    }

    #[test]
    fn full_negotiation_flow() {
        let mut m = PppStateMachine::new(5);
        assert_eq!(m.state(), PppState::Closed);
        m.start().unwrap();
        assert_eq!(m.state(), PppState::Establishing);
        m.on_lcp_config_ack().unwrap();
        assert_eq!(m.state(), PppState::Authenticating);
        m.on_auth_success().unwrap();
        assert_eq!(m.state(), PppState::Networking);
        let assigned = ip(172, 16, 0, 100);
        m.on_ipcp_config_ack(assigned).unwrap();
        assert_eq!(m.state(), PppState::Connected);
        assert_eq!(m.assigned_ip(), Some(assigned));
    }

    // --- Frame encode/decode tests ---

    #[test]
    fn encode_basic_frame() {
        let frame = PppFrame::new(PPP_LCP, alloc::vec![0x01, 0x02, 0x03]);
        let enc = frame.encode();
        // Flag | 0xC0 0x21 | 0x01 0x02 0x03 | 0x00 0x00 | Flag
        assert_eq!(
            enc,
            alloc::vec![0x7E, 0xC0, 0x21, 0x01, 0x02, 0x03, 0x1F, 0x4C, 0x7E]
        );
    }

    #[test]
    /// 31 DR-05 判据：RFC 1662 Appendix 已知向量 "123456789" → 0x906E。
    #[test]
    fn test_fcs16_rfc1662_vector() {
        assert_eq!(fcs16(b"123456789"), 0x906E);
    }

    #[test]
    fn decode_basic_frame() {
        // 31-review：FCS 用真实值 0x4C1F（LE=1F 4C，与 encode_basic_frame 配对），
        // 非 0x0000 旧占位——此前漏 #[test] 为死测试；补属性使其真正跑解码路径。
        let raw: &[u8] = &[0x7E, 0xC0, 0x21, 0x01, 0x02, 0x03, 0x1F, 0x4C, 0x7E];
        let frame = PppFrame::decode(raw).unwrap();
        assert_eq!(frame.protocol, PPP_LCP);
        assert_eq!(frame.data, alloc::vec![0x01, 0x02, 0x03]);
    }

    #[test]
    fn encode_decode_roundtrip() {
        let frame = PppFrame::new(PPP_IPCP, alloc::vec![0x10, 0x20, 0x30, 0x40]);
        let enc = frame.encode();
        let decoded = PppFrame::decode(&enc).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn encode_escapes_flag_byte_in_data() {
        // Data contains 0x7E -> 0x7D 0x5E。31-review（B3）：FCS 现按原始数据算，
        // 旧硬编码向量（FCS=52 39）失效——改为结构断言 + roundtrip 解码恢复。
        let frame = PppFrame::new(PPP_IP, alloc::vec![0x7E, 0x41]);
        let enc = frame.encode();
        assert!(
            enc.windows(2).any(|w| w == [0x7D, 0x5E]),
            "0x7E 未转义为 7D 5E: {:?}",
            enc
        );
        let decoded = PppFrame::decode(&enc).expect("roundtrip decode");
        assert_eq!(decoded.data, alloc::vec![0x7E, 0x41]);
    }

    #[test]
    fn encode_escapes_escape_byte_in_data() {
        // Data contains 0x7D -> 0x7D 0x5D（同上，roundtrip 验证）。
        let frame = PppFrame::new(PPP_IP, alloc::vec![0x7D, 0x42]);
        let enc = frame.encode();
        assert!(
            enc.windows(2).any(|w| w == [0x7D, 0x5D]),
            "0x7D 未转义为 7D 5D: {:?}",
            enc
        );
        let decoded = PppFrame::decode(&enc).expect("roundtrip decode");
        assert_eq!(decoded.data, alloc::vec![0x7D, 0x42]);
    }

    #[test]
    fn decode_unescapes_flag_byte() {
        // 31-review（B3）：用 encode 产出有效 FCS 帧（旧硬编码 FCS=0x0000 现被实校验拒）。
        let raw = PppFrame::new(PPP_IP, alloc::vec![0x7E, 0x41]).encode();
        let frame = PppFrame::decode(&raw).unwrap();
        assert_eq!(frame.protocol, PPP_IP);
        assert_eq!(frame.data, alloc::vec![0x7E, 0x41]);
    }

    #[test]
    fn decode_unescapes_escape_byte() {
        let raw = PppFrame::new(PPP_IP, alloc::vec![0x7D, 0x42]).encode();
        let frame = PppFrame::decode(&raw).unwrap();
        assert_eq!(frame.data, alloc::vec![0x7D, 0x42]);
    }

    #[test]
    fn decode_rejects_bad_fcs() {
        // 31-review（B3）：实校验判据——篡改 FCS 必须被拒
        //（旧实现 body=`let _ = 0;` 空操作，坏帧静默接受）。
        let mut raw = PppFrame::new(PPP_LCP, alloc::vec![0x01, 0x02, 0x03]).encode();
        // 布局：[FLAG, content..., fcs_lo, fcs_hi, FLAG] —— 翻转 fcs_lo。
        let fcs_lo_idx = raw.len() - 3;
        raw[fcs_lo_idx] ^= 0xFF;
        assert_eq!(
            PppFrame::decode(&raw).unwrap_err(),
            CellularError::PppNegotiationFailed
        );
    }

    #[test]
    fn roundtrip_with_escaped_bytes() {
        let data = alloc::vec![0x7E, 0x7D, 0x01, 0x7E, 0x02, 0x7D, 0x7D];
        let frame = PppFrame::new(PPP_PAP, data.clone());
        let enc = frame.encode();
        let decoded = PppFrame::decode(&enc).unwrap();
        assert_eq!(decoded.protocol, PPP_PAP);
        assert_eq!(decoded.data, data);
    }

    #[test]
    fn decode_rejects_missing_flags() {
        // No leading/trailing flag.
        let raw: &[u8] = &[0xC0, 0x21, 0x01, 0x02, 0x00, 0x00];
        assert_eq!(
            PppFrame::decode(raw).unwrap_err(),
            CellularError::PppNegotiationFailed
        );
    }

    #[test]
    fn decode_rejects_too_short() {
        let raw: &[u8] = &[0x7E, 0x7E];
        assert_eq!(
            PppFrame::decode(raw).unwrap_err(),
            CellularError::DialFailed
        );
    }

    #[test]
    fn decode_rejects_truncated_escape() {
        // Escape at end of data with no following byte.
        let raw: &[u8] = &[0x7E, 0xC0, 0x21, 0x7D, 0x00, 0x00, 0x7E];
        assert_eq!(
            PppFrame::decode(raw).unwrap_err(),
            CellularError::PppNegotiationFailed
        );
    }

    #[test]
    fn decode_rejects_invalid_escape_pair() {
        // 0x7D followed by an invalid byte (not 0x5E or 0x5D).
        let raw: &[u8] = &[0x7E, 0xC0, 0x21, 0x7D, 0xFF, 0x00, 0x00, 0x7E];
        assert_eq!(
            PppFrame::decode(raw).unwrap_err(),
            CellularError::PppNegotiationFailed
        );
    }

    #[test]
    fn encode_empty_data() {
        let frame = PppFrame::new(PPP_CHAP, alloc::vec![]);
        let enc = frame.encode();
        assert_eq!(enc, alloc::vec![0x7E, 0xC2, 0x23, 0xC4, 0xE5, 0x7E]);
        let decoded = PppFrame::decode(&enc).unwrap();
        assert_eq!(decoded, frame);
    }
}
