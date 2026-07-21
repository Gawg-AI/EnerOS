//! MQTT v3.1.1 报文编解码（Control Packet Codec）.
//!
//! 实现 14 种控制报文的编码与解码（MQTT v3.1.1 §2/§3）。
//! 所有报文由固定头（1 字节类型+标志 + 变长剩余长度）+ 可变头 + 负载构成。

use alloc::string::String;
use alloc::vec::Vec;

use crate::error::MqttError;
use crate::qos::QoS;
use crate::will::LastWill;

/// 报文类型常量（MQTT v3.1.1 §2.2.1）.
pub mod packet_type {
    pub const CONNECT: u8 = 1;
    pub const CONNACK: u8 = 2;
    pub const PUBLISH: u8 = 3;
    pub const PUBACK: u8 = 4;
    pub const PUBREC: u8 = 5;
    pub const PUBREL: u8 = 6;
    pub const PUBCOMP: u8 = 7;
    pub const SUBSCRIBE: u8 = 8;
    pub const SUBACK: u8 = 9;
    pub const UNSUBSCRIBE: u8 = 10;
    pub const UNSUBACK: u8 = 11;
    pub const PINGREQ: u8 = 12;
    pub const PINGRESP: u8 = 13;
    pub const DISCONNECT: u8 = 14;
}

/// CONNECT 报文.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectPacket {
    /// 客户端 ID.
    pub client_id: String,
    /// 用户名（可选）.
    pub username: Option<String>,
    /// 密码（可选，明文，D18）.
    pub password: Option<Vec<u8>>,
    /// 遗嘱消息（可选）.
    pub will: Option<LastWill>,
    /// 心跳间隔（秒）.
    pub keep_alive_secs: u16,
    /// 是否清除会话.
    pub clean_session: bool,
}

/// CONNACK 报文.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnackPacket {
    /// 会话存在标志.
    pub session_present: bool,
    /// 连接返回码（0=接受，1~5=拒绝）.
    pub return_code: u8,
}

/// PUBLISH 报文.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishPacket {
    /// 发布 Topic.
    pub topic: String,
    /// 报文 ID（QoS 1/2 必有，QoS 0 为 None）.
    pub packet_id: Option<u16>,
    /// QoS 等级.
    pub qos: QoS,
    /// 是否保留.
    pub retain: bool,
    /// 是否为重复.
    pub dup: bool,
    /// 负载.
    pub payload: Vec<u8>,
}

/// SUBSCRIBE 报文.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscribePacket {
    /// 报文 ID.
    pub packet_id: u16,
    /// 订阅 Topic 列表（Topic 过滤器 + QoS）.
    pub topics: Vec<(String, QoS)>,
}

/// SUBACK 报文.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubackPacket {
    /// 报文 ID.
    pub packet_id: u16,
    /// 返回码列表（0~2=QoS 等级，128=失败）.
    pub return_codes: Vec<u8>,
}

/// UNSUBSCRIBE 报文.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsubscribePacket {
    /// 报文 ID.
    pub packet_id: u16,
    /// 取消订阅 Topic 列表.
    pub topics: Vec<String>,
}

/// MQTT 控制报文枚举（14 种）.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MqttPacket {
    /// CONNECT（客户端 → Broker）.
    Connect(ConnectPacket),
    /// CONNACK（Broker → 客户端）.
    Connack(ConnackPacket),
    /// PUBLISH（双向）.
    Publish(PublishPacket),
    /// PUBACK（QoS 1 ACK，参数为 Packet ID）.
    Puback(u16),
    /// PUBREC（QoS 2 第 2 步，参数为 Packet ID）.
    Pubrec(u16),
    /// PUBREL（QoS 2 第 3 步，参数为 Packet ID）.
    Pubrel(u16),
    /// PUBCOMP（QoS 2 第 4 步，参数为 Packet ID）.
    Pubcomp(u16),
    /// SUBSCRIBE（客户端 → Broker）.
    Subscribe(SubscribePacket),
    /// SUBACK（Broker → 客户端）.
    Suback(SubackPacket),
    /// UNSUBSCRIBE（客户端 → Broker）.
    Unsubscribe(UnsubscribePacket),
    /// UNSUBACK（Broker → 客户端，参数为 Packet ID）.
    Unsuback(u16),
    /// PINGREQ.
    Pingreq,
    /// PINGRESP.
    Pingresp,
    /// DISCONNECT.
    Disconnect,
}

// ===== 内部辅助 =====

/// 编码剩余长度（变长整数 1~4 字节，MQTT v3.1.1 §2.2.3）.
pub fn encode_remaining_length(mut len: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut byte = (len % 128) as u8;
        len /= 128;
        if len > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if len == 0 {
            break;
        }
    }
    out
}

/// 解码剩余长度，返回 (长度, 消耗字节数).
pub fn decode_remaining_length(bytes: &[u8]) -> Result<(usize, usize), MqttError> {
    let mut multiplier: usize = 1;
    let mut value: usize = 0;
    let mut idx = 0usize;
    while idx < bytes.len() {
        if idx >= 4 {
            // 超过 4 字节 → 非法（MQTT v3.1.1 §2.2.3 最多 4 字节）
            return Err(MqttError::PacketDecodeError);
        }
        let byte = bytes[idx];
        value += ((byte & 0x7F) as usize) * multiplier;
        multiplier *= 128;
        idx += 1;
        if (byte & 0x80) == 0 {
            return Ok((value, idx));
        }
    }
    Err(MqttError::PacketDecodeError)
}

/// 编码长度前缀字符串（2 字节大端长度 + 字符串字节）.
fn encode_string(s: &str, out: &mut Vec<u8>) {
    let len = s.len();
    if len > 0xFFFF {
        // 超长字符串按 0xFFFF 截断（避免 panic）
        out.push(0xFF);
        out.push(0xFF);
        out.extend_from_slice(&s.as_bytes()[..0xFFFF]);
        return;
    }
    out.push(((len >> 8) & 0xFF) as u8);
    out.push((len & 0xFF) as u8);
    out.extend_from_slice(s.as_bytes());
}

/// 读取长度前缀字符串，返回 (字符串, 消耗字节数).
fn decode_string(bytes: &[u8]) -> Result<(String, usize), MqttError> {
    if bytes.len() < 2 {
        return Err(MqttError::PacketDecodeError);
    }
    let len = ((bytes[0] as usize) << 8) | (bytes[1] as usize);
    if bytes.len() < 2 + len {
        return Err(MqttError::PacketDecodeError);
    }
    let s =
        String::from_utf8(bytes[2..2 + len].to_vec()).map_err(|_| MqttError::PacketDecodeError)?;
    Ok((s, 2 + len))
}

/// 编码 2 字节大端无符号整数.
fn encode_u16(value: u16, out: &mut Vec<u8>) {
    out.push(((value >> 8) & 0xFF) as u8);
    out.push((value & 0xFF) as u8);
}

/// 读取 2 字节大端无符号整数.
fn decode_u16(bytes: &[u8]) -> Result<(u16, usize), MqttError> {
    if bytes.len() < 2 {
        return Err(MqttError::PacketDecodeError);
    }
    let v = ((bytes[0] as u16) << 8) | (bytes[1] as u16);
    Ok((v, 2))
}

// ===== 子报文编码 =====

fn encode_connect(p: &ConnectPacket) -> Vec<u8> {
    let mut vh = Vec::new();
    // 协议名 "MQTT"
    encode_string("MQTT", &mut vh);
    // 协议级别 4（MQTT v3.1.1）
    vh.push(0x04);
    // 连接标志
    let mut flags: u8 = 0;
    if p.clean_session {
        flags |= 0x02;
    }
    if let Some(will) = &p.will {
        flags |= 0x04; // Will Flag
        flags |= (will.qos.as_u8() & 0x03) << 3; // Will QoS
        if will.retain {
            flags |= 0x20; // Will Retain
        }
    }
    if p.username.is_some() {
        flags |= 0x80; // User Name Flag
    }
    if p.password.is_some() {
        flags |= 0x40; // Password Flag
    }
    vh.push(flags);
    // Keep Alive
    encode_u16(p.keep_alive_secs, &mut vh);
    // Payload: ClientID
    encode_string(&p.client_id, &mut vh);
    // Will Topic + Will Payload
    if let Some(will) = &p.will {
        encode_string(&will.topic, &mut vh);
        // Will Payload 为长度前缀字节流
        let plen = will.payload.len();
        encode_u16(plen as u16, &mut vh);
        vh.extend_from_slice(&will.payload);
    }
    // Username
    if let Some(user) = &p.username {
        encode_string(user, &mut vh);
    }
    // Password
    if let Some(pass) = &p.password {
        let plen = pass.len();
        encode_u16(plen as u16, &mut vh);
        vh.extend_from_slice(pass);
    }
    vh
}

fn encode_publish(p: &PublishPacket) -> Vec<u8> {
    let mut vh = Vec::new();
    encode_string(&p.topic, &mut vh);
    if let Some(id) = p.packet_id {
        encode_u16(id, &mut vh);
    }
    vh.extend_from_slice(&p.payload);
    vh
}

fn encode_subscribe(p: &SubscribePacket) -> Vec<u8> {
    let mut vh = Vec::new();
    encode_u16(p.packet_id, &mut vh);
    for (topic, qos) in &p.topics {
        encode_string(topic, &mut vh);
        vh.push(qos.as_u8());
    }
    vh
}

fn encode_unsubscribe(p: &UnsubscribePacket) -> Vec<u8> {
    let mut vh = Vec::new();
    encode_u16(p.packet_id, &mut vh);
    for topic in &p.topics {
        encode_string(topic, &mut vh);
    }
    vh
}

// ===== 子报文解码 =====

fn decode_connack(bytes: &[u8]) -> Result<ConnackPacket, MqttError> {
    if bytes.len() < 2 {
        return Err(MqttError::PacketDecodeError);
    }
    Ok(ConnackPacket {
        session_present: (bytes[0] & 0x01) != 0,
        return_code: bytes[1],
    })
}

fn decode_publish(
    bytes: &[u8],
    qos_bits: u8,
    dup: bool,
    retain: bool,
) -> Result<PublishPacket, MqttError> {
    let (topic, consumed) = decode_string(bytes)?;
    let mut offset = consumed;
    let qos = QoS::from_u8(qos_bits).unwrap_or(QoS::AtMostOnce);
    let packet_id = if qos != QoS::AtMostOnce {
        let (id, c) = decode_u16(&bytes[offset..])?;
        offset += c;
        Some(id)
    } else {
        None
    };
    let payload = bytes[offset..].to_vec();
    Ok(PublishPacket {
        topic,
        packet_id,
        qos,
        retain,
        dup,
        payload,
    })
}

fn decode_subscribe(bytes: &[u8]) -> Result<SubscribePacket, MqttError> {
    let (packet_id, c) = decode_u16(bytes)?;
    let mut offset = c;
    let mut topics = Vec::new();
    while offset < bytes.len() {
        let (topic, c2) = decode_string(&bytes[offset..])?;
        offset += c2;
        if offset >= bytes.len() {
            return Err(MqttError::PacketDecodeError);
        }
        let qos_byte = bytes[offset];
        offset += 1;
        let qos = QoS::from_u8(qos_byte).unwrap_or(QoS::AtMostOnce);
        topics.push((topic, qos));
    }
    Ok(SubscribePacket { packet_id, topics })
}

fn decode_suback(bytes: &[u8]) -> Result<SubackPacket, MqttError> {
    let (packet_id, c) = decode_u16(bytes)?;
    let return_codes = bytes[c..].to_vec();
    Ok(SubackPacket {
        packet_id,
        return_codes,
    })
}

fn decode_unsubscribe(bytes: &[u8]) -> Result<UnsubscribePacket, MqttError> {
    let (packet_id, c) = decode_u16(bytes)?;
    let mut offset = c;
    let mut topics = Vec::new();
    while offset < bytes.len() {
        let (topic, c2) = decode_string(&bytes[offset..])?;
        offset += c2;
        topics.push(topic);
    }
    Ok(UnsubscribePacket { packet_id, topics })
}

// ===== 公共 API =====

/// 编码报文为字节流.
pub fn encode(packet: &MqttPacket) -> Vec<u8> {
    let (ptype, flags, vh_payload): (u8, u8, Vec<u8>) = match packet {
        MqttPacket::Connect(p) => (packet_type::CONNECT, 0u8, encode_connect(p)),
        MqttPacket::Connack(p) => {
            let mut vh = Vec::new();
            vh.push(if p.session_present { 0x01 } else { 0x00 });
            vh.push(p.return_code);
            (packet_type::CONNACK, 0u8, vh)
        }
        MqttPacket::Publish(p) => {
            let mut flags: u8 = 0;
            if p.dup {
                flags |= 0x08;
            }
            flags |= (p.qos.as_u8() & 0x03) << 1;
            if p.retain {
                flags |= 0x01;
            }
            (packet_type::PUBLISH, flags, encode_publish(p))
        }
        MqttPacket::Puback(id) => {
            let mut vh = Vec::new();
            encode_u16(*id, &mut vh);
            (packet_type::PUBACK, 0u8, vh)
        }
        MqttPacket::Pubrec(id) => {
            let mut vh = Vec::new();
            encode_u16(*id, &mut vh);
            (packet_type::PUBREC, 0u8, vh)
        }
        MqttPacket::Pubrel(id) => {
            let mut vh = Vec::new();
            encode_u16(*id, &mut vh);
            // PUBREL 固定标志位 0b0010
            (packet_type::PUBREL, 0x02, vh)
        }
        MqttPacket::Pubcomp(id) => {
            let mut vh = Vec::new();
            encode_u16(*id, &mut vh);
            (packet_type::PUBCOMP, 0u8, vh)
        }
        MqttPacket::Subscribe(p) => {
            // SUBSCRIBE 固定标志位 0b0010
            (packet_type::SUBSCRIBE, 0x02, encode_subscribe(p))
        }
        MqttPacket::Suback(p) => {
            let mut vh = Vec::new();
            encode_u16(p.packet_id, &mut vh);
            vh.extend_from_slice(&p.return_codes);
            (packet_type::SUBACK, 0u8, vh)
        }
        MqttPacket::Unsubscribe(p) => {
            // UNSUBSCRIBE 固定标志位 0b0010
            (packet_type::UNSUBSCRIBE, 0x02, encode_unsubscribe(p))
        }
        MqttPacket::Unsuback(id) => {
            let mut vh = Vec::new();
            encode_u16(*id, &mut vh);
            (packet_type::UNSUBACK, 0u8, vh)
        }
        MqttPacket::Pingreq => (packet_type::PINGREQ, 0u8, Vec::new()),
        MqttPacket::Pingresp => (packet_type::PINGRESP, 0u8, Vec::new()),
        MqttPacket::Disconnect => (packet_type::DISCONNECT, 0u8, Vec::new()),
    };

    let mut out = Vec::new();
    let first_byte = (ptype << 4) | (flags & 0x0F);
    out.push(first_byte);
    out.extend_from_slice(&encode_remaining_length(vh_payload.len()));
    out.extend_from_slice(&vh_payload);
    out
}

/// 从字节流解码一个完整报文.
///
/// 输入应为一个完整报文（含固定头 + 可变头 + 负载），函数内部解析剩余长度。
pub fn decode(bytes: &[u8]) -> Result<MqttPacket, MqttError> {
    if bytes.is_empty() {
        return Err(MqttError::PacketDecodeError);
    }
    let first_byte = bytes[0];
    let ptype = (first_byte >> 4) & 0x0F;
    let flags = first_byte & 0x0F;
    let (rem_len, header_consumed) = decode_remaining_length(&bytes[1..])?;
    let payload_start = 1 + header_consumed;
    if payload_start + rem_len > bytes.len() {
        return Err(MqttError::PacketDecodeError);
    }
    let vh = &bytes[payload_start..payload_start + rem_len];

    match ptype {
        packet_type::CONNECT => {
            // 解码 CONNECT（用于测试 roundtrip，实现完整解析）
            let mut offset = 0;
            let (_proto_name, c) = decode_string(&vh[offset..])?;
            offset += c;
            if offset >= vh.len() {
                return Err(MqttError::PacketDecodeError);
            }
            let _proto_level = vh[offset];
            offset += 1;
            if offset >= vh.len() {
                return Err(MqttError::PacketDecodeError);
            }
            let connect_flags = vh[offset];
            offset += 1;
            let (keep_alive_secs, c) = decode_u16(&vh[offset..])?;
            offset += c;
            let clean_session = (connect_flags & 0x02) != 0;
            let will_flag = (connect_flags & 0x04) != 0;
            let will_qos = QoS::from_u8((connect_flags >> 3) & 0x03).unwrap_or(QoS::AtMostOnce);
            let will_retain = (connect_flags & 0x20) != 0;
            let username_flag = (connect_flags & 0x80) != 0;
            let password_flag = (connect_flags & 0x40) != 0;
            let (client_id, c) = decode_string(&vh[offset..])?;
            offset += c;
            let mut will: Option<LastWill> = None;
            if will_flag {
                let (topic, c2) = decode_string(&vh[offset..])?;
                offset += c2;
                let (plen, c3) = decode_u16(&vh[offset..])?;
                offset += c3;
                if offset + plen as usize > vh.len() {
                    return Err(MqttError::PacketDecodeError);
                }
                let payload = vh[offset..offset + plen as usize].to_vec();
                offset += plen as usize;
                will = Some(LastWill {
                    topic,
                    payload,
                    qos: will_qos,
                    retain: will_retain,
                });
            }
            let mut username: Option<String> = None;
            if username_flag {
                let (user, c2) = decode_string(&vh[offset..])?;
                offset += c2;
                username = Some(user);
            }
            let mut password: Option<Vec<u8>> = None;
            if password_flag {
                let (plen, c2) = decode_u16(&vh[offset..])?;
                offset += c2;
                if offset + plen as usize > vh.len() {
                    return Err(MqttError::PacketDecodeError);
                }
                password = Some(vh[offset..offset + plen as usize].to_vec());
                offset += plen as usize;
            }
            let _ = offset;
            Ok(MqttPacket::Connect(ConnectPacket {
                client_id,
                username,
                password,
                will,
                keep_alive_secs,
                clean_session,
            }))
        }
        packet_type::CONNACK => {
            let connack = decode_connack(vh)?;
            Ok(MqttPacket::Connack(connack))
        }
        packet_type::PUBLISH => {
            let dup = (flags & 0x08) != 0;
            let qos_bits = (flags >> 1) & 0x03;
            let retain = (flags & 0x01) != 0;
            let publish = decode_publish(vh, qos_bits, dup, retain)?;
            Ok(MqttPacket::Publish(publish))
        }
        packet_type::PUBACK => {
            let (id, _) = decode_u16(vh)?;
            Ok(MqttPacket::Puback(id))
        }
        packet_type::PUBREC => {
            let (id, _) = decode_u16(vh)?;
            Ok(MqttPacket::Pubrec(id))
        }
        packet_type::PUBREL => {
            let (id, _) = decode_u16(vh)?;
            Ok(MqttPacket::Pubrel(id))
        }
        packet_type::PUBCOMP => {
            let (id, _) = decode_u16(vh)?;
            Ok(MqttPacket::Pubcomp(id))
        }
        packet_type::SUBSCRIBE => {
            let sub = decode_subscribe(vh)?;
            Ok(MqttPacket::Subscribe(sub))
        }
        packet_type::SUBACK => {
            let suback = decode_suback(vh)?;
            Ok(MqttPacket::Suback(suback))
        }
        packet_type::UNSUBSCRIBE => {
            let unsub = decode_unsubscribe(vh)?;
            Ok(MqttPacket::Unsubscribe(unsub))
        }
        packet_type::UNSUBACK => {
            let (id, _) = decode_u16(vh)?;
            Ok(MqttPacket::Unsuback(id))
        }
        packet_type::PINGREQ => Ok(MqttPacket::Pingreq),
        packet_type::PINGRESP => Ok(MqttPacket::Pingresp),
        packet_type::DISCONNECT => Ok(MqttPacket::Disconnect),
        _ => Err(MqttError::PacketDecodeError),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remaining_length_encode_decode() {
        // 0
        assert_eq!(encode_remaining_length(0), vec![0x00]);
        // 127
        assert_eq!(encode_remaining_length(127), vec![0x7F]);
        // 128
        assert_eq!(encode_remaining_length(128), vec![0x80, 0x01]);
        // 16383
        assert_eq!(encode_remaining_length(16383), vec![0xFF, 0x7F]);
        // 16384
        assert_eq!(encode_remaining_length(16384), vec![0x80, 0x80, 0x01]);

        // 解码往返
        for v in [0usize, 127, 128, 16383, 16384, 2097151, 2097152] {
            let enc = encode_remaining_length(v);
            let (dec, consumed) = decode_remaining_length(&enc).unwrap();
            assert_eq!(dec, v);
            assert_eq!(consumed, enc.len());
        }
    }

    #[test]
    fn test_connect_encode_first_byte() {
        let p = ConnectPacket {
            client_id: String::from("client-1"),
            username: None,
            password: None,
            will: None,
            keep_alive_secs: 60,
            clean_session: true,
        };
        let bytes = encode(&MqttPacket::Connect(p));
        // 第一字节：CONNECT(1) << 4 = 0x10，flags=0
        assert_eq!(bytes[0], 0x10);
    }

    #[test]
    fn test_connack_decode() {
        // CONNACK: 0x20 0x02 0x00 0x00（session_present=false, return_code=0）
        let bytes = [0x20u8, 0x02, 0x00, 0x00];
        let pkt = decode(&bytes).unwrap();
        assert!(matches!(pkt, MqttPacket::Connack(_)));
        if let MqttPacket::Connack(c) = pkt {
            assert!(!c.session_present);
            assert_eq!(c.return_code, 0);
        }
    }

    #[test]
    fn test_publish_qos0_roundtrip() {
        let p = PublishPacket {
            topic: String::from("sensor/temp"),
            packet_id: None,
            qos: QoS::AtMostOnce,
            retain: false,
            dup: false,
            payload: vec![1, 2, 3],
        };
        let bytes = encode(&MqttPacket::Publish(p.clone()));
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, MqttPacket::Publish(p));
    }

    #[test]
    fn test_publish_qos1_roundtrip() {
        let p = PublishPacket {
            topic: String::from("sensor/temp"),
            packet_id: Some(42),
            qos: QoS::AtLeastOnce,
            retain: true,
            dup: false,
            payload: vec![0xDE, 0xAD],
        };
        let bytes = encode(&MqttPacket::Publish(p.clone()));
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, MqttPacket::Publish(p));
    }

    #[test]
    fn test_subscribe_encode_type_byte() {
        let p = SubscribePacket {
            packet_id: 1,
            topics: vec![(String::from("a/b"), QoS::AtLeastOnce)],
        };
        let bytes = encode(&MqttPacket::Subscribe(p));
        // 第一字节：SUBSCRIBE(8) << 4 = 0x80，flags=0b0010 → 0x82
        assert_eq!(bytes[0], 0x82);
    }

    #[test]
    fn test_pingreq_pingresp_roundtrip() {
        let bytes = encode(&MqttPacket::Pingreq);
        assert_eq!(bytes, vec![0xC0, 0x00]);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, MqttPacket::Pingreq);

        let bytes = encode(&MqttPacket::Pingresp);
        assert_eq!(bytes, vec![0xD0, 0x00]);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, MqttPacket::Pingresp);
    }
}
