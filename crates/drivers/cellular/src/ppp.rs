//! PPP (Point-to-Point Protocol) dial-up negotiation (v0.30.1).
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

    /// Encode this frame to HDLC-encoded bytes.
    ///
    /// Layout: `Flag | Protocol(2, BE) | Data(escaped) | FCS(2) | Flag`.
    ///
    /// The FCS is a simplified fixed `0x0000`. A complete implementation
    /// must compute CRC-16-CCITT over `Protocol + Data` and escape the FCS
    /// bytes alongside the data.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(HDLC_FLAG);
        // Protocol field, big-endian, not escaped (standard protocols never
        // contain 0x7E/0x7D).
        out.push((self.protocol >> 8) as u8);
        out.push((self.protocol & 0xFF) as u8);
        // Data with HDLC byte-stuffing.
        for &b in &self.data {
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
        // FCS: simplified fixed value (full impl: CRC-16-CCITT).
        out.push(0x00);
        out.push(0x00);
        out.push(HDLC_FLAG);
        out
    }

    /// Decode an HDLC-encoded frame.
    ///
    /// Expects `Flag | Protocol(2, BE) | Data(escaped) | FCS(2) | Flag`.
    /// The FCS is read but not validated (see `encode` caveat).
    pub fn decode(raw: &[u8]) -> Result<PppFrame, CellularError> {
        // Minimum: Flag + Protocol(2) + FCS(2) + Flag = 6 bytes.
        if raw.len() < 6 {
            return Err(CellularError::DialFailed);
        }
        if raw.first() != Some(&HDLC_FLAG) || raw.last() != Some(&HDLC_FLAG) {
            return Err(CellularError::PppNegotiationFailed);
        }
        let middle = &raw[1..raw.len() - 1];
        if middle.len() < 4 {
            return Err(CellularError::DialFailed);
        }
        let protocol = ((middle[0] as u16) << 8) | (middle[1] as u16);
        // Last 2 bytes = FCS (skipped), middle = escaped data.
        let escaped = &middle[2..middle.len() - 2];
        let mut data = Vec::with_capacity(escaped.len());
        let mut i = 0;
        while i < escaped.len() {
            let b = escaped[i];
            if b == HDLC_ESCAPE {
                if i + 1 >= escaped.len() {
                    return Err(CellularError::PppNegotiationFailed);
                }
                match escaped[i + 1] {
                    0x5E => data.push(HDLC_FLAG),
                    0x5D => data.push(HDLC_ESCAPE),
                    _ => return Err(CellularError::PppNegotiationFailed),
                }
                i += 2;
            } else {
                data.push(b);
                i += 1;
            }
        }
        Ok(PppFrame { protocol, data })
    }
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
            alloc::vec![0x7E, 0xC0, 0x21, 0x01, 0x02, 0x03, 0x00, 0x00, 0x7E]
        );
    }

    #[test]
    fn decode_basic_frame() {
        let raw: &[u8] = &[0x7E, 0xC0, 0x21, 0x01, 0x02, 0x03, 0x00, 0x00, 0x7E];
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
        // Data contains 0x7E -> 0x7D 0x5E
        let frame = PppFrame::new(PPP_IP, alloc::vec![0x7E, 0x41]);
        let enc = frame.encode();
        assert_eq!(
            enc,
            alloc::vec![0x7E, 0x00, 0x21, 0x7D, 0x5E, 0x41, 0x00, 0x00, 0x7E]
        );
    }

    #[test]
    fn encode_escapes_escape_byte_in_data() {
        // Data contains 0x7D -> 0x7D 0x5D
        let frame = PppFrame::new(PPP_IP, alloc::vec![0x7D, 0x42]);
        let enc = frame.encode();
        assert_eq!(
            enc,
            alloc::vec![0x7E, 0x00, 0x21, 0x7D, 0x5D, 0x42, 0x00, 0x00, 0x7E]
        );
    }

    #[test]
    fn decode_unescapes_flag_byte() {
        let raw: &[u8] = &[0x7E, 0x00, 0x21, 0x7D, 0x5E, 0x41, 0x00, 0x00, 0x7E];
        let frame = PppFrame::decode(raw).unwrap();
        assert_eq!(frame.protocol, PPP_IP);
        assert_eq!(frame.data, alloc::vec![0x7E, 0x41]);
    }

    #[test]
    fn decode_unescapes_escape_byte() {
        let raw: &[u8] = &[0x7E, 0x00, 0x21, 0x7D, 0x5D, 0x42, 0x00, 0x00, 0x7E];
        let frame = PppFrame::decode(raw).unwrap();
        assert_eq!(frame.data, alloc::vec![0x7D, 0x42]);
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
        assert_eq!(enc, alloc::vec![0x7E, 0xC2, 0x23, 0x00, 0x00, 0x7E]);
        let decoded = PppFrame::decode(&enc).unwrap();
        assert_eq!(decoded, frame);
    }
}
