//! Ethernet frame structure and encoding/decoding.
//!
//! Provides [`EthFrame`] for constructing and parsing standard Ethernet
//! frames (dst MAC + src MAC + EtherType + payload). FCS/CRC is
//! intentionally omitted: the hardware MAC strips FCS. VLAN 802.1Q support
//! (46 WP-46.1：GOOSE/SV 固定偏移解头缺口) lives in [`header_info`] /
//! [`encode_vlan`]——解析侧跳过 TPID+TCI 后 EtherType/载荷偏移自动 +4，
//! 编码侧在 src 与 EtherType 之间插入 TPID(0x8100)+TCI。

use alloc::vec::Vec;
use core::fmt;

use crate::error::NetError;

/// 802.1Q TPID（VLAN 标签标识，EtherType 位置出现）。
pub const TPID_8021Q: u16 = 0x8100;
/// 无 VLAN 以太网头长（dst 6 + src 6 + EtherType 2）。
pub const ETH_HDR_LEN: usize = 14;
/// 802.1Q 标签插入后的以太网头长（+TPID 2 + TCI 2）。
pub const VLAN_HDR_LEN: usize = 18;

/// 802.1Q VLAN 标签（TPID 固定 0x8100；TCI = PCP 3bit + DEI 1bit + VID 12bit）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VlanTag {
    /// TCI（Tag Control Information）原始值。
    pub tci: u16,
}

/// 解析帧头：返回 `(EtherType 字节偏移, VLAN 标签)`。
///
/// 无 VLAN → `(12, None)`；802.1Q 标记 → `(16, Some(tag))`；帧过短或
/// 标记残缺 → `None`（fail-closed：不猜）。
pub fn header_info(frame: &[u8]) -> Option<(usize, Option<VlanTag>)> {
    if frame.len() < ETH_HDR_LEN {
        return None;
    }
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    if ethertype == TPID_8021Q {
        if frame.len() < VLAN_HDR_LEN {
            return None;
        }
        let tci = u16::from_be_bytes([frame[14], frame[15]]);
        Some((16, Some(VlanTag { tci })))
    } else {
        Some((12, None))
    }
}

/// 帧的有效 EtherType（跳过 VLAN 标签；SV 0x88BA / GOOSE 0x88B8 等）。
pub fn net_ether_type(frame: &[u8]) -> Option<u16> {
    let (off, _) = header_info(frame)?;
    Some(u16::from_be_bytes([frame[off], frame[off + 1]]))
}

/// 载荷起始偏移（无 VLAN 14 / 802.1Q 标记 18）。
pub fn payload_offset(frame: &[u8]) -> Option<usize> {
    let (off, _) = header_info(frame)?;
    Some(off + 2)
}

/// 把无 VLAN 帧编码为 802.1Q 标记帧（TPID+TCI 插于 src 与 EtherType 之间）。
///
/// 输入本身已含 VLAN 标签或帧过短 → `None`（不二次嵌套、不猜）。
pub fn encode_vlan(frame: &[u8], tci: u16) -> Option<Vec<u8>> {
    let (_, vlan) = header_info(frame)?;
    if vlan.is_some() {
        return None;
    }
    let mut out = Vec::with_capacity(frame.len() + 4);
    out.extend_from_slice(&frame[..12]);
    out.extend_from_slice(&TPID_8021Q.to_be_bytes());
    out.extend_from_slice(&tci.to_be_bytes());
    out.extend_from_slice(&frame[12..]);
    Some(out)
}

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

    // ========================================================================
    // 46 WP-46.1：802.1Q VLAN 解析（SV/GOOSE 固定偏移解头的缺口补齐）
    // ========================================================================

    fn plain_frame(ethertype: u16) -> Vec<u8> {
        let mut f = vec![0x01, 0x0c, 0xcd, 0x04, 0x00, 0x01]; // dst
        f.extend_from_slice(&[0x02, 0, 0, 0, 0, 0x02]); // src
        f.extend_from_slice(&ethertype.to_be_bytes());
        f.extend_from_slice(&[0x40, 0x01, 0x00, 0x00]); // APPID + 保留
        f
    }

    #[test]
    fn test_header_info_plain() {
        let f = plain_frame(0x88BA);
        let (off, vlan) = header_info(&f).expect("header");
        assert_eq!(off, 12);
        assert!(vlan.is_none());
        assert_eq!(net_ether_type(&f), Some(0x88BA));
        assert_eq!(payload_offset(&f), Some(14));
    }

    #[test]
    fn test_header_info_vlan() {
        let f = plain_frame(0x88BA);
        let tagged = encode_vlan(&f, 0x002A).expect("encode_vlan"); // VID 42
        assert_eq!(tagged.len(), f.len() + 4);
        let (off, vlan) = header_info(&tagged).expect("header");
        assert_eq!(off, 16);
        let tag = vlan.expect("vlan tag");
        assert_eq!(tag.tci, 0x002A);
        assert_eq!(net_ether_type(&tagged), Some(0x88BA));
        assert_eq!(payload_offset(&tagged), Some(18));
        // 标签插入位置：TPID 在 src 后，EtherType 移位到 16。
        assert_eq!(&tagged[12..14], &TPID_8021Q.to_be_bytes());
        assert_eq!(&tagged[14..16], &0x002Au16.to_be_bytes());
        assert_eq!(&tagged[16..18], &0x88BAu16.to_be_bytes());
    }

    #[test]
    fn test_header_info_vlan_detect_only_0x8100() {
        // 非 0x8100 的 12 位偏移值按普通 EtherType 处理（不做嵌套判断）。
        let f = plain_frame(0x88B8);
        assert_eq!(header_info(&f).unwrap().0, 12);
        let (_, vlan) = header_info(&f).unwrap();
        assert!(vlan.is_none());
    }

    #[test]
    fn test_header_info_too_short() {
        assert_eq!(header_info(&[0u8; 13]), None);
        // 只有 14 字节的"看似 VLAN"帧（缺 TCI）→ None（fail-closed）。
        let mut f = vec![0u8; 14];
        f[12..14].copy_from_slice(&TPID_8021Q.to_be_bytes());
        assert_eq!(header_info(&f), None);
    }

    #[test]
    fn test_encode_vlan_no_double_tag() {
        let f = plain_frame(0x88BA);
        let tagged = encode_vlan(&f, 7).expect("encode_vlan");
        // 已标记帧不允许二次嵌套（不猜）。
        assert_eq!(encode_vlan(&tagged, 9), None);
        // 过短帧（<14B）→ None。
        assert_eq!(encode_vlan(&[0u8; 10], 9), None);
    }

    #[test]
    fn test_encode_vlan_roundtrip_preserves_payload() {
        let mut f = plain_frame(0x88BA);
        f.extend_from_slice(&[0x80, 0x02, 0x00, 0x01]); // SV PDU 头
        let tagged = encode_vlan(&f, 100).expect("encode_vlan");
        // 载荷区（VLAN 后偏移 18 起）与原始（14 起）逐字节一致。
        assert_eq!(&tagged[18..], &f[14..]);
        let (off, vlan) = header_info(&tagged).expect("header");
        assert_eq!(off, 16);
        assert!(vlan.is_some());
    }
}
