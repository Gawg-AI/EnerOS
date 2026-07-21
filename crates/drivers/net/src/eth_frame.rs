//! Ethernet frame structure and encoding/decoding.
//!
//! Provides [`EthFrame`] for constructing and parsing standard Ethernet
//! frames (dst MAC + src MAC + EtherType + payload). VLAN tags and FCS/CRC
//! are intentionally omitted: the hardware MAC strips FCS, and VLAN support
//! is deferred to the TCP/IP stack (v0.28.0+).

use alloc::vec::Vec;
use core::fmt;

use crate::error::NetError;

/// EtherType field values.
///
/// Only the most common types are enumerated; unknown values are preserved
/// via [`EtherType::Other`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EtherType {
    /// IPv4 (0x0800).
    Ipv4,
    /// IPv6 (0x86DD).
    Ipv6,
    /// ARP (0x0806).
    Arp,
    /// Any other EtherType value.
    Other(u16),
}

impl EtherType {
    /// Convert a raw `u16` to an [`EtherType`].
    pub const fn from_u16(val: u16) -> Self {
        match val {
            0x0800 => EtherType::Ipv4,
            0x86DD => EtherType::Ipv6,
            0x0806 => EtherType::Arp,
            _ => EtherType::Other(val),
        }
    }

    /// Convert an [`EtherType`] to a raw `u16`.
    pub const fn to_u16(&self) -> u16 {
        match self {
            EtherType::Ipv4 => 0x0800,
            EtherType::Ipv6 => 0x86DD,
            EtherType::Arp => 0x0806,
            EtherType::Other(v) => *v,
        }
    }
}

impl fmt::Display for EtherType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EtherType::Ipv4 => write!(f, "IPv4 (0x0800)"),
            EtherType::Ipv6 => write!(f, "IPv6 (0x86DD)"),
            EtherType::Arp => write!(f, "ARP (0x0806)"),
            EtherType::Other(v) => write!(f, "Other (0x{:04X})", v),
        }
    }
}

/// Standard Ethernet frame (no VLAN, no FCS).
///
/// Layout when encoded:
/// ```text
/// +----------+----------+----------+---------+
/// | dst_mac  | src_mac  | ethertype| payload |
/// | 6 bytes  | 6 bytes  | 2 bytes  | 0..N    |
/// +----------+----------+----------+---------+
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct EthFrame {
    /// Destination MAC address.
    pub dst_mac: [u8; 6],
    /// Source MAC address.
    pub src_mac: [u8; 6],
    /// EtherType field (identifies the upper-layer protocol).
    pub ethertype: EtherType,
    /// Frame payload (variable length).
    pub payload: Vec<u8>,
}

impl EthFrame {
    /// Create a new Ethernet frame.
    pub fn new(dst: [u8; 6], src: [u8; 6], ethertype: EtherType, payload: Vec<u8>) -> Self {
        Self {
            dst_mac: dst,
            src_mac: src,
            ethertype,
            payload,
        }
    }

    /// Encode the frame to a byte vector.
    ///
    /// Layout: dst_mac (6) + src_mac (6) + ethertype (2, big-endian) + payload.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(14 + self.payload.len());
        buf.extend_from_slice(&self.dst_mac);
        buf.extend_from_slice(&self.src_mac);
        let et = self.ethertype.to_u16();
        buf.push((et >> 8) as u8);
        buf.push((et & 0xff) as u8);
        buf.extend_from_slice(&self.payload);
        buf
    }

    /// Decode a byte slice into an [`EthFrame`].
    ///
    /// Returns [`NetError::FrameTooSmall`] if the input is shorter than
    /// 14 bytes (the minimum Ethernet header size).
    pub fn decode(data: &[u8]) -> Result<Self, NetError> {
        if data.len() < 14 {
            return Err(NetError::FrameTooSmall);
        }
        let mut dst_mac = [0u8; 6];
        dst_mac.copy_from_slice(&data[0..6]);
        let mut src_mac = [0u8; 6];
        src_mac.copy_from_slice(&data[6..12]);
        let ethertype = EtherType::from_u16(((data[12] as u16) << 8) | (data[13] as u16));
        let payload = Vec::from(&data[14..]);
        Ok(Self {
            dst_mac,
            src_mac,
            ethertype,
            payload,
        })
    }

    /// Returns `true` if the destination MAC is the broadcast address
    /// (FF:FF:FF:FF:FF:FF).
    pub fn is_broadcast(&self) -> bool {
        self.dst_mac == [0xff; 6]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_mac(a: u8) -> [u8; 6] {
        [a, a, a, a, a, a]
    }

    #[test]
    fn test_ethertype_from_u16_known() {
        assert_eq!(EtherType::from_u16(0x0800), EtherType::Ipv4);
        assert_eq!(EtherType::from_u16(0x86DD), EtherType::Ipv6);
        assert_eq!(EtherType::from_u16(0x0806), EtherType::Arp);
    }

    #[test]
    fn test_ethertype_from_u16_other() {
        assert_eq!(EtherType::from_u16(0x1234), EtherType::Other(0x1234));
        assert_eq!(EtherType::from_u16(0x0000), EtherType::Other(0x0000));
        assert_eq!(EtherType::from_u16(0xFFFF), EtherType::Other(0xFFFF));
    }

    #[test]
    fn test_ethertype_to_u16_known() {
        assert_eq!(EtherType::Ipv4.to_u16(), 0x0800);
        assert_eq!(EtherType::Ipv6.to_u16(), 0x86DD);
        assert_eq!(EtherType::Arp.to_u16(), 0x0806);
    }

    #[test]
    fn test_ethertype_to_u16_other() {
        assert_eq!(EtherType::Other(0x1234).to_u16(), 0x1234);
    }

    #[test]
    fn test_ethertype_roundtrip() {
        for val in [0x0800u16, 0x86DD, 0x0806, 0x1234, 0x0000, 0xFFFF] {
            let et = EtherType::from_u16(val);
            assert_eq!(et.to_u16(), val);
        }
    }

    #[test]
    fn test_ethertype_display() {
        assert_eq!(format!("{}", EtherType::Ipv4), "IPv4 (0x0800)");
        assert_eq!(format!("{}", EtherType::Ipv6), "IPv6 (0x86DD)");
        assert_eq!(format!("{}", EtherType::Arp), "ARP (0x0806)");
        assert_eq!(format!("{}", EtherType::Other(0x1234)), "Other (0x1234)");
    }

    #[test]
    fn test_ethframe_new() {
        let frame = EthFrame::new(
            sample_mac(1),
            sample_mac(2),
            EtherType::Ipv4,
            Vec::from(&[0xDE, 0xAD][..]),
        );
        assert_eq!(frame.dst_mac, sample_mac(1));
        assert_eq!(frame.src_mac, sample_mac(2));
        assert_eq!(frame.ethertype, EtherType::Ipv4);
        assert_eq!(frame.payload, vec![0xDE, 0xAD]);
    }

    #[test]
    fn test_ethframe_encode_basic() {
        let frame = EthFrame::new(
            [0x01, 0x02, 0x03, 0x04, 0x05, 0x06],
            [0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F],
            EtherType::Ipv4,
            Vec::from(&[0xAA, 0xBB, 0xCC][..]),
        );
        let encoded = frame.encode();
        assert_eq!(encoded.len(), 17);
        assert_eq!(&encoded[0..6], &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06]);
        assert_eq!(&encoded[6..12], &[0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F]);
        assert_eq!(&encoded[12..14], &[0x08, 0x00]); // big-endian
        assert_eq!(&encoded[14..], &[0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn test_ethframe_encode_empty_payload() {
        let frame = EthFrame::new(sample_mac(0), sample_mac(1), EtherType::Arp, Vec::new());
        let encoded = frame.encode();
        assert_eq!(encoded.len(), 14);
    }

    #[test]
    fn test_ethframe_decode_basic() {
        let data: Vec<u8> = vec![
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, // dst
            0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, // src
            0x08, 0x00, // ethertype IPv4
            0xAA, 0xBB, // payload
        ];
        let frame = EthFrame::decode(&data).expect("decode failed");
        assert_eq!(frame.dst_mac, [0x01, 0x02, 0x03, 0x04, 0x05, 0x06]);
        assert_eq!(frame.src_mac, [0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F]);
        assert_eq!(frame.ethertype, EtherType::Ipv4);
        assert_eq!(frame.payload, vec![0xAA, 0xBB]);
    }

    #[test]
    fn test_ethframe_decode_empty_payload() {
        let data: Vec<u8> = vec![
            0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, // dst (broadcast)
            0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, // src
            0x08, 0x06, // ethertype ARP
        ];
        let frame = EthFrame::decode(&data).expect("decode failed");
        assert!(frame.payload.is_empty());
        assert_eq!(frame.ethertype, EtherType::Arp);
    }

    #[test]
    fn test_ethframe_decode_too_short() {
        let data: Vec<u8> = vec![0x00; 13];
        let result = EthFrame::decode(&data);
        assert_eq!(result, Err(NetError::FrameTooSmall));
    }

    #[test]
    fn test_ethframe_decode_exactly_14() {
        let data: Vec<u8> = vec![
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x86,
            0xDD, // IPv6
        ];
        let frame = EthFrame::decode(&data).expect("decode failed");
        assert_eq!(frame.ethertype, EtherType::Ipv6);
        assert!(frame.payload.is_empty());
    }

    #[test]
    fn test_ethframe_decode_zero_bytes() {
        let data: Vec<u8> = vec![];
        let result = EthFrame::decode(&data);
        assert_eq!(result, Err(NetError::FrameTooSmall));
    }

    #[test]
    fn test_ethframe_roundtrip() {
        let original = EthFrame::new(
            [0x11, 0x22, 0x33, 0x44, 0x55, 0x66],
            [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF],
            EtherType::Ipv6,
            Vec::from(&[0x01, 0x02, 0x03, 0x04, 0x05][..]),
        );
        let encoded = original.encode();
        let decoded = EthFrame::decode(&encoded).expect("decode failed");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_ethframe_roundtrip_empty_payload() {
        let original = EthFrame::new(sample_mac(1), sample_mac(2), EtherType::Arp, Vec::new());
        let encoded = original.encode();
        let decoded = EthFrame::decode(&encoded).expect("decode failed");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_ethframe_roundtrip_large_payload() {
        let payload = Vec::from(&[0xAB; 1500][..]);
        let original = EthFrame::new(sample_mac(1), sample_mac(2), EtherType::Ipv4, payload);
        let encoded = original.encode();
        assert_eq!(encoded.len(), 14 + 1500);
        let decoded = EthFrame::decode(&encoded).expect("decode failed");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_ethframe_roundtrip_other_ethertype() {
        let original = EthFrame::new(
            sample_mac(1),
            sample_mac(2),
            EtherType::Other(0x88CC),
            Vec::from(&[0x01][..]),
        );
        let encoded = original.encode();
        let decoded = EthFrame::decode(&encoded).expect("decode failed");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_is_broadcast_true() {
        let frame = EthFrame::new([0xFF; 6], sample_mac(1), EtherType::Ipv4, Vec::new());
        assert!(frame.is_broadcast());
    }

    #[test]
    fn test_is_broadcast_false() {
        let frame = EthFrame::new(
            [0x01, 0x02, 0x03, 0x04, 0x05, 0x06],
            sample_mac(1),
            EtherType::Ipv4,
            Vec::new(),
        );
        assert!(!frame.is_broadcast());
    }

    #[test]
    fn test_is_broadcast_partial_ff() {
        // Not all bytes are 0xFF, so not broadcast.
        let frame = EthFrame::new(
            [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00],
            sample_mac(1),
            EtherType::Ipv4,
            Vec::new(),
        );
        assert!(!frame.is_broadcast());
    }

    #[test]
    fn test_encode_big_endian_ethertype() {
        let frame = EthFrame::new(
            sample_mac(0),
            sample_mac(1),
            EtherType::Other(0x1234),
            Vec::new(),
        );
        let encoded = frame.encode();
        // 0x1234 big-endian → 0x12, 0x34
        assert_eq!(encoded[12], 0x12);
        assert_eq!(encoded[13], 0x34);
    }

    #[test]
    fn test_decode_multicast() {
        // Multicast MAC: first byte has bit 0 set (odd).
        let data: Vec<u8> = vec![
            0x01, 0x00, 0x5E, 0x00, 0x00, 0x01, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x08, 0x00,
            0x42,
        ];
        let frame = EthFrame::decode(&data).expect("decode failed");
        assert_eq!(frame.dst_mac[0] & 0x01, 0x01); // multicast bit
        assert!(!frame.is_broadcast());
    }
}
