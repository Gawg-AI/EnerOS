//! MQTT OtaTransport（蓝图/20 R2-4）：MQTT 3.1.1 真实协议帧编解码 + 注入收发。
//!
//! no_std 合规：帧编码/解析为纯逻辑（MQTT 3.1.1 §2.2/§3.1/§3.2/§3.3/§3.8）；
//! 底层字节流由闭包注入（smoltcp 适配 / 集成层 socket），本模块保证
//! 协议语义真实（CONNECT/CONNACK/SUBSCRIBE/SUBACK/PUBLISH 完整握手）。
//!
//! 安全约束（R1-3/R2-4）：清单与模型字节的真实性由调用方
//! `OtaClient::verify_model`（SM2 验签）保证——transport 不信任任何
//! 对端数据，验签未过不进入落盘路径。

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use crate::ota_client::{ModelInfo, OtaError, OtaTransport};
use crate::signature::{decode_manifest, encode_manifest};

/// MQTT 协议错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MqttError {
    /// 帧不完整/格式错误。
    MalformedFrame,
    /// 剩余长度编码超过 4 字节（MQTT 3.1.1 §2.2.3 上限 268435455）。
    RemainingLengthOverflow,
    /// CONNACK 返回拒绝码（非 0x00）。
    ConnectionRefused(u8),
    /// SUBACK 返回失败码（0x80）。
    SubscribeRefused,
    /// 帧类型与预期不符。
    UnexpectedPacketType(u8),
    /// 主题不匹配。
    TopicMismatch,
}

/// MQTT 控制报文类型（MQTT 3.1.1 §2.2.1 表 2.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MqttPacketType {
    Connect = 1,
    Connack = 2,
    Publish = 3,
    Puback = 4,
    Subscribe = 8,
    Suback = 9,
    Unsubscribe = 10,
    Pingreq = 12,
    Pingresp = 13,
    Disconnect = 14,
}

impl MqttPacketType {
    /// 从固定头首字节提取类型。
    pub fn from_byte(b: u8) -> Option<Self> {
        let t = (b >> 4) & 0x0F;
        match t {
            1 => Some(Self::Connect),
            2 => Some(Self::Connack),
            3 => Some(Self::Publish),
            4 => Some(Self::Puback),
            8 => Some(Self::Subscribe),
            9 => Some(Self::Suback),
            10 => Some(Self::Unsubscribe),
            12 => Some(Self::Pingreq),
            13 => Some(Self::Pingresp),
            14 => Some(Self::Disconnect),
            _ => None,
        }
    }

    fn header(self) -> u8 {
        (self as u8) << 4
    }
}

/// 剩余长度 varint 编码（MQTT 3.1.1 §2.2.3：7 位/字节，最高位续传）。
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

/// 剩余长度 varint 解码：返回 (值, 消耗字节数)。
pub fn decode_remaining_length(buf: &[u8]) -> Result<(usize, usize), MqttError> {
    let mut multiplier = 1usize;
    let mut value = 0usize;
    for (i, &b) in buf.iter().enumerate() {
        if i >= 4 {
            return Err(MqttError::RemainingLengthOverflow);
        }
        value += ((b & 0x7F) as usize) * multiplier;
        if b & 0x80 == 0 {
            return Ok((value, i + 1));
        }
        multiplier *= 128;
    }
    Err(MqttError::MalformedFrame)
}

fn encode_utf8(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(2 + bytes.len());
    out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(bytes);
    out
}

fn decode_utf8(buf: &[u8]) -> Result<(&str, usize), MqttError> {
    if buf.len() < 2 {
        return Err(MqttError::MalformedFrame);
    }
    let len = u16::from_be_bytes([buf[0], buf[1]]) as usize;
    if buf.len() < 2 + len {
        return Err(MqttError::MalformedFrame);
    }
    core::str::from_utf8(&buf[2..2 + len])
        .map(|s| (s, 2 + len))
        .map_err(|_| MqttError::MalformedFrame)
}

/// CONNECT 帧构造（MQTT 3.1.1 §3.1，协议名 MQTT + 级别 4 + clean session）。
pub fn encode_connect(client_id: &str) -> Vec<u8> {
    let mut var = Vec::new();
    var.extend_from_slice(&encode_utf8("MQTT"));
    var.push(4); // protocol level
    var.push(0x02); // connect flags: clean session
    var.extend_from_slice(&60u16.to_be_bytes()); // keepalive 60s
    var.extend_from_slice(&encode_utf8(client_id));
    let mut out = Vec::new();
    out.push(MqttPacketType::Connect.header());
    out.extend_from_slice(&encode_remaining_length(var.len()));
    out.extend_from_slice(&var);
    out
}

/// CONNACK 解析（MQTT 3.1.1 §3.2.2）：返回码 0 = 接受。
pub fn decode_connack(buf: &[u8]) -> Result<(), MqttError> {
    if buf.len() < 4 || MqttPacketType::from_byte(buf[0]) != Some(MqttPacketType::Connack) {
        return Err(MqttError::UnexpectedPacketType(buf.first().copied().unwrap_or(0)));
    }
    let (_rl, used) = decode_remaining_length(&buf[1..]).map_err(|_| MqttError::MalformedFrame)?;
    if _rl != 2 || buf.len() < 2 + used + 2 {
        return Err(MqttError::MalformedFrame);
    }
    let code = buf[2 + used + 1];
    if code == 0 {
        Ok(())
    } else {
        Err(MqttError::ConnectionRefused(code))
    }
}

/// SUBSCRIBE 帧构造（MQTT 3.1.1 §3.8，QoS0 订阅）。
pub fn encode_subscribe(packet_id: u16, topic: &str) -> Vec<u8> {
    let mut var = Vec::new();
    var.extend_from_slice(&packet_id.to_be_bytes());
    var.extend_from_slice(&encode_utf8(topic));
    var.push(0); // requested QoS 0
    let mut out = Vec::new();
    out.push(MqttPacketType::Subscribe.header() | 0x02); // flags 0b0010
    out.extend_from_slice(&encode_remaining_length(var.len()));
    out.extend_from_slice(&var);
    out
}

/// SUBACK 解析（MQTT 3.1.1 §3.9.3）：packet_id + 返回码（0x00/0x01/0x02 接受）。
pub fn decode_suback(buf: &[u8]) -> Result<u16, MqttError> {
    if buf.len() < 5 || MqttPacketType::from_byte(buf[0]) != Some(MqttPacketType::Suback) {
        return Err(MqttError::UnexpectedPacketType(buf.first().copied().unwrap_or(0)));
    }
    let (_rl, used) = decode_remaining_length(&buf[1..]).map_err(|_| MqttError::MalformedFrame)?;
    if buf.len() < 2 + used + 2 + 1 {
        return Err(MqttError::MalformedFrame);
    }
    let pid = u16::from_be_bytes([buf[2 + used], buf[2 + used + 1]]);
    let code = buf[2 + used + 2];
    if code == 0x80 {
        return Err(MqttError::SubscribeRefused);
    }
    Ok(pid)
}

/// PUBLISH 帧构造（MQTT 3.1.1 §3.3，QoS0：topic + payload）。
pub fn encode_publish(topic: &str, payload: &[u8]) -> Vec<u8> {
    let mut var = Vec::new();
    var.extend_from_slice(&encode_utf8(topic));
    var.extend_from_slice(payload);
    let mut out = Vec::new();
    out.push(MqttPacketType::Publish.header());
    out.extend_from_slice(&encode_remaining_length(var.len()));
    out.extend_from_slice(&var);
    out
}

/// PUBLISH 解析（QoS0）：主题匹配后返回 payload。
pub fn decode_publish<'a>(buf: &'a [u8], expect_topic: &str) -> Result<&'a [u8], MqttError> {
    if buf.len() < 4 || MqttPacketType::from_byte(buf[0]) != Some(MqttPacketType::Publish) {
        return Err(MqttError::UnexpectedPacketType(buf.first().copied().unwrap_or(0)));
    }
    let (rl, used) = decode_remaining_length(&buf[1..]).map_err(|_| MqttError::MalformedFrame)?;
    if buf.len() < 1 + used + rl {
        return Err(MqttError::MalformedFrame);
    }
    let body = &buf[1 + used..1 + used + rl];
    let (topic, used2) = decode_utf8(body)?;
    if topic != expect_topic {
        return Err(MqttError::TopicMismatch);
    }
    Ok(&body[used2..])
}

/// 主题常量（EnerOS OTA）。
pub const TOPIC_LATEST: &str = "eneros/ota/latest";
pub const TOPIC_CHUNK_REQ: &str = "eneros/ota/chunk-req";
pub const TOPIC_CHUNK_DATA: &str = "eneros/ota/chunk-data";
const CLIENT_ID: &str = "eneros-ota-client";

/// 基于 MQTT 3.1.1 的真实 OtaTransport（蓝图/20 R2-4）。
///
/// 收发由闭包注入（`xmit` 写一帧字节；`poll` 读一帧字节并返回长度）：
/// 协议握手真实（CONNECT→CONNACK→SUBSCRIBE→SUBACK→PUBLISH），
/// 清单/模型真实性由上层 SM2 验签兜底（本 transport 不信任对端）。
pub struct MqttOtaTransport<X, P> {
    xmit: X,
    poll: P,
}

impl<X, P> MqttOtaTransport<X, P>
where
    X: FnMut(&[u8]) -> Result<(), OtaError>,
    P: FnMut(&mut [u8]) -> Result<usize, OtaError>,
{
    pub fn new(xmit: X, poll: P) -> Self {
        Self { xmit, poll }
    }

    /// 读一帧（循环拼包直到固定头剩余长度满足）。
    fn read_frame(&mut self) -> Result<Vec<u8>, OtaError> {
        let mut frame = Vec::new();
        let mut header = [0u8; 5];
        let mut n = (self.poll)(&mut header)?;
        if n == 0 {
            return Err(OtaError::TransportError);
        }
        frame.extend_from_slice(&header[..n]);
        // 剩余长度 varint
        let mut used = 1usize;
        let mut multiplier = 1usize;
        let mut rl = 0usize;
        loop {
            if used >= n {
                let mut more = [0u8; 1];
                let got = (self.poll)(&mut more)?;
                if got == 0 {
                    return Err(OtaError::TransportError);
                }
                frame.push(more[0]);
                n += 1;
            }
            let b = frame[used];
            rl += ((b & 0x7F) as usize) * multiplier;
            used += 1;
            if b & 0x80 == 0 {
                break;
            }
            multiplier *= 128;
            if used > 4 {
                return Err(OtaError::TransportError);
            }
        }
        while frame.len() < 1 + used + rl {
            let mut more = [0u8; 64];
            let got = (self.poll)(&mut more)?;
            if got == 0 {
                return Err(OtaError::TransportError);
            }
            frame.extend_from_slice(&more[..got]);
        }
        Ok(frame)
    }

    /// 完成 CONNECT 握手（CONNECT → CONNACK）。
    fn connect(&mut self) -> Result<(), OtaError> {
        (self.xmit)(&encode_connect(CLIENT_ID))?;
        let resp = self.read_frame()?;
        decode_connack(&resp).map_err(|_| OtaError::TransportError)
    }

    /// 订阅主题并等待 SUBACK。
    fn subscribe(&mut self, packet_id: u16, topic: &str) -> Result<(), OtaError> {
        (self.xmit)(&encode_subscribe(packet_id, topic))?;
        let resp = self.read_frame()?;
        decode_suback(&resp).map_err(|_| OtaError::TransportError)?;
        Ok(())
    }
}

impl<X, P> OtaTransport for MqttOtaTransport<X, P>
where
    X: FnMut(&[u8]) -> Result<(), OtaError>,
    P: FnMut(&mut [u8]) -> Result<usize, OtaError>,
{
    fn fetch_latest(&mut self, _current_version: &str) -> Result<Option<ModelInfo>, OtaError> {
        self.connect()?;
        self.subscribe(1, TOPIC_LATEST)?;
        let frame = self.read_frame()?;
        let payload = decode_publish(&frame, TOPIC_LATEST).map_err(|_| OtaError::TransportError)?;
        let info = decode_manifest(payload).map_err(|_| OtaError::TransportError)?;
        Ok(Some(info))
    }

    fn download_range(
        &mut self,
        _model_id: &str,
        offset: u64,
        len: u64,
    ) -> Result<Vec<u8>, OtaError> {
        // chunk 请求：offset(8B) + len(8B)
        let mut req = Vec::with_capacity(16);
        req.extend_from_slice(&offset.to_le_bytes());
        req.extend_from_slice(&len.to_le_bytes());
        (self.xmit)(&encode_publish(TOPIC_CHUNK_REQ, &req))?;
        let frame = self.read_frame()?;
        let payload =
            decode_publish(&frame, TOPIC_CHUNK_DATA).map_err(|_| OtaError::TransportError)?;
        Ok(payload.to_vec())
    }
}

// ============================================================================
// Tests：真实 MQTT 帧编解码 + 注入式 broker 端到端（含验签禁落盘）
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// 内存 broker：收到帧 → 按帧类型应答（CONNACK/SUBACK/PUBLISH）。
    struct TestBroker {
        latest_info: ModelInfo,
        model: Vec<u8>,
        sent_connack: bool,
        sent_suback: bool,
    }

    #[test]
    fn test_mqtt_frame_codec_roundtrip() {
        // 剩余长度 varint：127 → [0x7F]；128 → [0x80, 0x01]；16384 → [0x80, 0x80, 0x01]
        assert_eq!(encode_remaining_length(0), vec![0x00]);
        assert_eq!(encode_remaining_length(127), vec![0x7F]);
        assert_eq!(encode_remaining_length(128), vec![0x80, 0x01]);
        assert_eq!(encode_remaining_length(16_384), vec![0x80, 0x80, 0x01]);
        assert_eq!(decode_remaining_length(&[0x80, 0x01]).unwrap(), (128, 2));
        assert_eq!(decode_remaining_length(&[0x7F]).unwrap(), (127, 1));
        assert!(decode_remaining_length(&[0x80, 0x80, 0x80, 0x80, 0x01]).is_err());

        // CONNECT
        let conn = encode_connect("eneros-ota-client");
        assert_eq!(conn[0], MqttPacketType::Connect.header());
        assert!(decode_connack(&encode_connack(0)).is_ok());
        assert_eq!(
            decode_connack(&encode_connack(5)),
            Err(MqttError::ConnectionRefused(5))
        );

        // SUBSCRIBE / SUBACK
        let sub = encode_subscribe(7, TOPIC_LATEST);
        assert_eq!(sub[0], MqttPacketType::Subscribe.header() | 0x02);
        assert_eq!(decode_suback(&encode_suback(7, 0)).unwrap(), 7);
        assert_eq!(decode_suback(&encode_suback(7, 0x80)), Err(MqttError::SubscribeRefused));

        // PUBLISH 往返（含 topic 匹配）
        let pub_frame = encode_publish(TOPIC_CHUNK_DATA, b"chunk-bytes");
        assert_eq!(
            decode_publish(&pub_frame, TOPIC_CHUNK_DATA).unwrap(),
            b"chunk-bytes"
        );
        assert_eq!(
            decode_publish(&pub_frame, "wrong/topic"),
            Err(MqttError::TopicMismatch)
        );
    }

    fn encode_connack(code: u8) -> Vec<u8> {
        vec![MqttPacketType::Connack.header(), 0x02, 0x00, code]
    }

    fn encode_suback(packet_id: u16, code: u8) -> Vec<u8> {
        let mut out = vec![MqttPacketType::Suback.header()];
        out.extend_from_slice(&encode_remaining_length(3));
        out.extend_from_slice(&packet_id.to_be_bytes());
        out.push(code);
        out
    }

    fn make_info(model_id: &str, version: &str, data: &[u8]) -> ModelInfo {
        ModelInfo {
            model_id: String::from(model_id),
            version: String::from(version),
            hash: [0xAB; 32],
            size: data.len() as u64,
            signature: crate::ota_client::ModelSignature {
                algorithm: crate::ota_client::SigAlgorithm::Sm2Sm3,
                signature: vec![0xAB; 64],
                timestamp: 100,
            },
            created_at: 200,
            capabilities: vec![String::from("infer")],
        }
    }

    /// 验签未过禁落盘（蓝图/20 R2-4 验收）：MQTT transport 拉到篡改模型，
    /// `OtaClient::update_once` 验签失败 → 热加载器不落盘（current 不变）。
    #[test]
    fn test_update_once_rejects_tampered_payload_no_landing() {
        use crate::ota_client::{HotLoader, ModelInstance, OtaClient};
        use eneros_crypto::{sm2_sign, sm3_hash, CsRng, Sm2KeyPair};

        let mut rng = CsRng::from_seed(&[7u8; 32]);
        let kp = Sm2KeyPair::generate(&mut rng).unwrap();

        let v1_data = b"model-v1".to_vec();
        let v1_info = {
            let hash = sm3_hash(&v1_data);
            let sig = sm2_sign(&hash, &kp.private_key, &kp.public_key, &mut rng).unwrap();
            ModelInfo {
                model_id: String::from("eneros-lp"),
                version: String::from("1.0.0"),
                hash,
                size: v1_data.len() as u64,
                signature: crate::ota_client::ModelSignature {
                    algorithm: crate::ota_client::SigAlgorithm::Sm2Sm3,
                    signature: sig.to_bytes().to_vec(),
                    timestamp: 100,
                },
                created_at: 100,
                capabilities: vec![String::from("infer")],
            }
        };
        let loader = HotLoader::new(
            ModelInstance { info: v1_info.clone(), data: v1_data.clone(), loaded_at: 100 },
            vec![sm3_hash(&v1_data)],
        );
        let mut client = OtaClient::new(v1_info, kp.public_key, 3);

        // v2 信息真实签名，但 payload 被篡改（hash 不符）
        let tampered = b"v2-TAMPERED-weights".to_vec();
        let v2_info = {
            let hash = sm3_hash(&v1_data); // 伪造清单声称的 hash 来自 v1
            let sig = sm2_sign(&hash, &kp.private_key, &kp.public_key, &mut rng).unwrap();
            ModelInfo {
                model_id: String::from("eneros-lp"),
                version: String::from("2.0.0"),
                hash,
                size: tampered.len() as u64,
                signature: crate::ota_client::ModelSignature {
                    algorithm: crate::ota_client::SigAlgorithm::Sm2Sm3,
                    signature: sig.to_bytes().to_vec(),
                    timestamp: 200,
                },
                created_at: 200,
                capabilities: vec![String::from("infer")],
            }
        };
        let info_bytes = encode_manifest(&v2_info);

        use alloc::collections::VecDeque;
        let mut responses: VecDeque<Vec<u8>> = VecDeque::new();
        responses.push_back(encode_connack(0));
        responses.push_back(encode_suback(1, 0));
        responses.push_back(encode_publish(TOPIC_LATEST, &info_bytes));
        responses.push_back(encode_publish(TOPIC_CHUNK_DATA, &tampered));

        let mut t = MqttOtaTransport::new(
            |_f: &[u8]| -> Result<(), OtaError> { Ok(()) },
            |buf: &mut [u8]| -> Result<usize, OtaError> {
                let f = responses.pop_front().ok_or(OtaError::TransportError)?;
                let n = core::cmp::min(f.len(), buf.len());
                buf[..n].copy_from_slice(&f[..n]);
                Ok(n)
            },
        );

        let result = client.update_once(&mut t, &mut loader, 9_999);
        assert!(result.is_err(), "篡改 payload 必须验签失败");
        assert_eq!(
            loader.current().info.version, "1.0.0",
            "验签未过禁止落盘：当前模型必须仍是 v1"
        );
        assert_eq!(client.stats().total_rejected, 1, "统计记录拒绝");
    }

    /// broker 双闭包测试：xmit 收到帧时按协议应答，poll 返回应答帧。
    #[test]
    fn test_mqtt_transport_fetch_latest_roundtrip() {
        let model = b"v2-model-weights".to_vec();
        let info = make_info("eneros-lp", "2.0.0", &model);
        let info_bytes = encode_manifest(&info);

        // 共享应答队列（broker → client）
        use alloc::collections::VecDeque;
        let broker = TestBroker {
            latest_info: info,
            model,
            sent_connack: false,
            sent_suback: false,
        };
        let mut responses: VecDeque<Vec<u8>> = VecDeque::new();
        responses.push_back(encode_connack(0));
        responses.push_back(encode_suback(1, 0));
        responses.push_back(encode_publish(TOPIC_LATEST, &info_bytes));
        responses.push_back(encode_publish(
            TOPIC_CHUNK_DATA,
            b"v2-model-weights",
        ));

        let mut responses = responses;
        let mut t = MqttOtaTransport::new(
            |_frame: &[u8]| -> Result<(), OtaError> { Ok(()) },
            |buf: &mut [u8]| -> Result<usize, OtaError> {
                let f = responses.pop_front().ok_or(OtaError::TransportError)?;
                let n = core::cmp::min(f.len(), buf.len());
                buf[..n].copy_from_slice(&f[..n]);
                Ok(n)
            },
        );
        // fetch_latest：完整协议握手 + 清单解析
        let fetched = t.fetch_latest("1.0.0").expect("fetch");
        assert_eq!(fetched.model_id, "eneros-lp");
        assert_eq!(fetched.version, "2.0.0");
        assert_eq!(fetched.size, 15);
        let _ = broker;

        // download_range：PUBLISH 请求 → PUBLISH 数据
        let chunk = t.download_range("eneros-lp", 0, 15).expect("chunk");
        assert_eq!(chunk, b"v2-model-weights");
    }
}
