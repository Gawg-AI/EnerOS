//! QoS 1/2 在途消息持久化（v0.221.0，B2-04）。
//!
//! 此前 QoS 1/2 未确认消息仅存内存（D15：断网即丢失）。本模块提供：
//!
//! 1. [`QosStore`] trait — 在途消息持久化抽象（save/load/remove/list）；
//! 2. [`FsQosStore`] — 基于 eneros-fs `FileSystem` 的落盘实现（重启补传）；
//! 3. [`InMemoryQosStore`] — 内存实现（测试/无持久化目标）。
//!
//! 持久化格式：每行 `packet_id,payload_b64,topic,qos,state`（文本行，
//! 与 fs crate 的字节接口兼容；base64 解码由调用方提供——为 no_std
//! 简单起见，payload 以十六进制编码存储）。

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::client::PendingAck;
use crate::error::MqttError;

/// 一条待补传的在途 QoS 1/2 消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingMessage {
    /// 报文 ID。
    pub packet_id: u16,
    /// Topic。
    pub topic: String,
    /// Payload（原始字节）。
    pub payload: Vec<u8>,
    /// QoS 等级（1 或 2）。
    pub qos: u8,
    /// 当前确认状态。
    pub state: PendingAck,
}

/// QoS 在途消息持久化抽象。
///
/// 实现者负责把未确认消息落盘，使设备重启/断网后可按报文 ID 补传
/// （QoS1 消息零丢失）。
pub trait QosStore {
    /// 保存一条在途消息（幂等：同 packet_id 覆盖）。
    fn save(&mut self, msg: &PendingMessage) -> Result<(), MqttError>;
    /// 按 packet_id 移除（确认完成后调用）。
    fn remove(&mut self, packet_id: u16) -> Result<(), MqttError>;
    /// 加载全部在途消息（启动恢复用）。
    fn load_all(&mut self) -> Vec<PendingMessage>;
    /// 是否包含指定 packet_id。
    fn contains(&mut self, packet_id: u16) -> bool;
}

/// 内存实现（无持久化目标时使用；重启即失）。
#[derive(Debug, Default)]
pub struct InMemoryQosStore {
    /// packet_id → 消息。
    messages: BTreeMap<u16, PendingMessage>,
}

impl InMemoryQosStore {
    /// 创建空内存存储。
    pub fn new() -> Self {
        Self::default()
    }
}

impl QosStore for InMemoryQosStore {
    fn save(&mut self, msg: &PendingMessage) -> Result<(), MqttError> {
        self.messages.insert(msg.packet_id, msg.clone());
        Ok(())
    }
    fn remove(&mut self, packet_id: u16) -> Result<(), MqttError> {
        self.messages.remove(&packet_id);
        Ok(())
    }
    fn load_all(&mut self) -> Vec<PendingMessage> {
        self.messages.values().cloned().collect()
    }
    fn contains(&mut self, packet_id: u16) -> bool {
        self.messages.contains_key(&packet_id)
    }
}

// ============================================================================
// 字节编解码（fs 行格式）
// ============================================================================

/// 十六进制字符表。
const HEX_DIGITS: [u8; 16] = *b"0123456789abcdef";

/// 编码一条在途消息为文本行。
///
/// 格式：`<packet_id>,<qos>,<state>,<topic>,<payload_hex>`。
/// state 编码：1=WaitingPuback，2=WaitingPubrec，3=WaitingPubcomp。
pub fn encode_line(msg: &PendingMessage) -> Vec<u8> {
    let state = match msg.state {
        PendingAck::WaitingPuback => 1u8,
        PendingAck::WaitingPubrec { .. } => 2,
        PendingAck::WaitingPubcomp { .. } => 3,
    };
    let mut line = String::new();
    line.push_str(&msg.packet_id.to_string());
    line.push(',');
    line.push_str(&msg.qos.to_string());
    line.push(',');
    line.push_str(&state.to_string());
    line.push(',');
    line.push_str(&msg.topic);
    line.push(',');
    for b in &msg.payload {
        line.push(HEX_DIGITS[(b >> 4) as usize] as char);
        line.push(HEX_DIGITS[(b & 0xF) as usize] as char);
    }
    line.push('\n');
    line.into_bytes()
}

/// 解码一行文本为在途消息；格式非法返回 `None`。
pub fn decode_line(line: &[u8]) -> Option<PendingMessage> {
    let text = core::str::from_utf8(line).ok()?;
    let text = text.trim_end_matches(['\r', '\n']);
    let mut parts = text.splitn(5, ',');
    let packet_id: u16 = parts.next()?.parse().ok()?;
    let qos: u8 = parts.next()?.parse().ok()?;
    let state_code: u8 = parts.next()?.parse().ok()?;
    let topic = parts.next()?.to_string();
    let payload_hex = parts.next()?;
    let state = match state_code {
        1 => PendingAck::WaitingPuback,
        2 => PendingAck::WaitingPubrec { packet_id },
        3 => PendingAck::WaitingPubcomp { packet_id },
        _ => return None,
    };
    // 十六进制解码 payload。
    let payload_bytes = payload_hex.as_bytes();
    if payload_bytes.len() % 2 != 0 {
        return None;
    }
    let mut payload = Vec::with_capacity(payload_bytes.len() / 2);
    for pair in payload_bytes.chunks(2) {
        let hi = hex_val(pair[0])?;
        let lo = hex_val(pair[1])?;
        payload.push((hi << 4) | lo);
    }
    Some(PendingMessage {
        packet_id,
        topic,
        payload,
        qos,
        state,
    })
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// 基于 eneros-fs 的落盘实现。
///
/// 文件路径：`/mqtt/pending.txt`。每次 save 全量重写（在途消息数量
/// 有限，简单可靠）；load_all 解析整文件。
pub struct FsQosStore<F> {
    /// 底层文件系统。
    fs: F,
    /// 持久化文件路径。
    path: String,
}

impl<F> FsQosStore<F> {
    /// 创建 fs 后端。
    pub fn new(fs: F, path: &str) -> Self {
        Self {
            fs,
            path: String::from(path),
        }
    }
}

impl<F: eneros_fs::fs_trait::FileSystem> QosStore for FsQosStore<F> {
    fn save(&mut self, msg: &PendingMessage) -> Result<(), MqttError> {
        // 读现有全部行，覆盖/追加目标条目后全量写回。
        let mut lines: Vec<PendingMessage> = self.load_all();
        lines.retain(|m| m.packet_id != msg.packet_id);
        lines.push(msg.clone());
        self.write_all(&lines)
    }

    fn remove(&mut self, packet_id: u16) -> Result<(), MqttError> {
        let lines: Vec<PendingMessage> = self
            .load_all()
            .into_iter()
            .filter(|m| m.packet_id != packet_id)
            .collect();
        self.write_all(&lines)
    }

    fn load_all(&mut self) -> Vec<PendingMessage> {
        // v0.221.0：经 FileSystem::read_file 路径级读取（trait 新增方法）。
        let mut out = Vec::new();
        let Ok(data) = self.fs.read_file(&self.path) else {
            return out;
        };
        for line in data.split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            if let Some(msg) = decode_line(line) {
                out.push(msg);
            }
        }
        out
    }

    fn contains(&mut self, packet_id: u16) -> bool {
        self.load_all().iter().any(|m| m.packet_id == packet_id)
    }
}

impl<F: eneros_fs::fs_trait::FileSystem> FsQosStore<F> {
    /// 全量写回（截断重写）。
    fn write_all(&mut self, messages: &[PendingMessage]) -> Result<(), MqttError> {
        let mut bytes = Vec::new();
        for m in messages {
            bytes.extend_from_slice(&encode_line(m));
        }
        self.fs
            .write_file(&self.path, &bytes)
            .map_err(|_| MqttError::TransportError)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(packet_id: u16, topic: &str, payload: &[u8], qos: u8) -> PendingMessage {
        PendingMessage {
            packet_id,
            topic: String::from(topic),
            payload: payload.to_vec(),
            qos,
            state: PendingAck::WaitingPuback,
        }
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let m = msg(42, "soc/report", &[0xDE, 0xAD, 0xBE, 0xEF], 1);
        let line = encode_line(&m);
        let decoded = decode_line(&line).expect("decode");
        assert_eq!(decoded, m);
    }

    #[test]
    fn test_decode_invalid_rejected() {
        assert!(decode_line(b"not-a-line\n").is_none());
        assert!(decode_line(b"1,1,9,topic,00\n").is_none(), "非法 state");
        assert!(decode_line(b"x,1,1,t,00\n").is_none(), "非法 id");
    }

    #[test]
    fn test_in_memory_store_roundtrip() {
        let mut store = InMemoryQosStore::new();
        let m = msg(1, "t", b"hello", 1);
        store.save(&m).expect("save");
        assert!(store.contains(1));
        let all = store.load_all();
        assert_eq!(all.len(), 1);
        store.remove(1).expect("remove");
        assert!(!store.contains(1));
    }

    #[test]
    fn test_in_memory_store_overwrite() {
        let mut store = InMemoryQosStore::new();
        store.save(&msg(1, "t1", b"a", 1)).expect("save");
        store.save(&msg(1, "t2", b"b", 1)).expect("overwrite");
        let all = store.load_all();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].topic, "t2");
    }

    #[test]
    fn test_hex_payload_with_utf8_topic() {
        let m = msg(7, "kwh/累计", &[0x01, 0x02], 2);
        let line = encode_line(&m);
        let decoded = decode_line(&line).expect("decode");
        assert_eq!(decoded.topic, "kwh/累计");
        assert_eq!(decoded.payload, vec![0x01, 0x02]);
    }

    // ===== FsQosStore（MockFs 后端） =====

    #[test]
    fn test_fs_store_roundtrip() {
        let fs = eneros_fs::MockFileSystem::new();
        let mut store = FsQosStore::new(fs, "/mqtt/pending.txt");
        let m1 = msg(1, "soc", &[0x01, 0x02], 1);
        let m2 = msg(2, "power", &[0xFF], 2);
        store.save(&m1).expect("save 1");
        store.save(&m2).expect("save 2");
        let all = store.load_all();
        assert_eq!(all.len(), 2);
        assert!(store.contains(1));
        store.remove(1).expect("remove");
        let all = store.load_all();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].packet_id, 2);
    }

    #[test]
    fn test_fs_store_overwrite_same_id() {
        let fs = eneros_fs::MockFileSystem::new();
        let mut store = FsQosStore::new(fs, "/mqtt/pending.txt");
        store.save(&msg(1, "t1", b"a", 1)).expect("save");
        store.save(&msg(1, "t2", b"b", 1)).expect("overwrite");
        let all = store.load_all();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].topic, "t2");
    }

    #[test]
    fn test_fs_store_missing_file_empty() {
        let fs = eneros_fs::MockFileSystem::new();
        let mut store = FsQosStore::new(fs, "/nonexistent.txt");
        assert!(store.load_all().is_empty());
    }
}
