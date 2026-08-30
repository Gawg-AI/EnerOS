//! 蓝图 61 [02] §3.2/§3.3：`DomainEvent`（业务与控制事实）+ `BlobRef`
//! （大对象引用）。
//!
//! 热控制路径的字符串/复杂业务事实走 DomainEvent（数值遥测走
//! [`super::event::ObservationEvent`]）；超限大对象不进 IPC 热路径——
//! 以 [`BlobRef`] 引用（读取必须再校验 size/digest/ACL/租户/有效期）。
//!
//! # 大小关系（编译期断言，[06] §4.7）
//!
//! - DomainEvent 逻辑上限 = 134B 头 + 1024B payload = 1158B；
//! - agent-sdk chunk 分页上限 = `CHUNK_COUNT_MAX × CHUNK_DATA_MAX` =
//!   16 × 104 = 1664B——DomainEvent 最大帧可单消息分页承载；
//! - BlobRef 定长 76B，单 chunk 可承载（≤104B）。

use alloc::vec::Vec;

use sha2::{Digest, Sha256};

/// classification 分类码（[02] §3.2：Public/Internal/Confidential/
/// Restricted——Restricted 默认禁止云同步）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classification {
    Public,
    Internal,
    Confidential,
    Restricted,
}

impl Classification {
    pub const fn code(self) -> u8 {
        match self {
            Self::Public => 0,
            Self::Internal => 1,
            Self::Confidential => 2,
            Self::Restricted => 3,
        }
    }

    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Public),
            1 => Some(Self::Internal),
            2 => Some(Self::Confidential),
            3 => Some(Self::Restricted),
            _ => None,
        }
    }
}

/// 逻辑消息头（定长 134B，小端；字段集 [02] §3.2 冻结）。
pub const DOMAIN_HEADER_LEN: usize = 134;
/// payload 上限（[06] §4.2：超过必须 BlobRef）。
pub const DOMAIN_PAYLOAD_MAX: usize = 1024;
/// 逻辑消息最大 wire 尺寸。
pub const DOMAIN_EVENT_MAX_WIRE: usize = DOMAIN_HEADER_LEN + DOMAIN_PAYLOAD_MAX;
/// BlobRef wire 定长。
pub const BLOB_REF_WIRE_LEN: usize = 76;

// —— 编译期大小关系断言（[06] §4.7 退出 gate）——
const _: () = assert!(DOMAIN_HEADER_LEN == 134);
const _: () = assert!(DOMAIN_PAYLOAD_MAX == 1024);
const _: () = assert!(DOMAIN_EVENT_MAX_WIRE == 1158);
const _: () = assert!(BLOB_REF_WIRE_LEN == 76);
// 与 chunk 分页上限 1664B 的关系（对侧常量在 agent-sdk；此处按 [06] §4.2
// 冻结数字互锁，agent-sdk 侧有镜像断言——两常量任一漂移即编译失败）。
pub const CHUNK_LOGICAL_CAP: usize = 16 * 104; // 1664
const _: () = assert!(DOMAIN_EVENT_MAX_WIRE <= CHUNK_LOGICAL_CAP);
// ObservationEvent（128B）远小于单 chunk 载荷（104×2）。
const _: () = assert!(super::event::OBSERVATION_WIRE_LEN < 2 * 104);

/// 域事件（不可变事实；payload 携带 schema 注册的编码体）。
#[derive(Debug, Clone, PartialEq)]
pub struct DomainEvent {
    pub event_id: [u8; 16],
    pub tenant_id: u32,
    pub site_id: u32,
    pub producer_id: u32,
    pub producer_epoch: u64,
    pub sequence: u64,
    pub occurred_at_ns: u64,
    pub recorded_at_ns: u64,
    pub schema_id: u32,
    pub schema_version: u16,
    pub classification: Classification,
    pub flags: u8,
    pub correlation_id: [u8; 16],
    pub causation_id: [u8; 16],
    pub payload: Vec<u8>,
    /// payload SHA-256（[06] §4.2：所有逻辑消息有 payload 摘要）。
    payload_sha256: [u8; 32],
}

impl DomainEvent {
    /// 构造（payload 上限 + 摘要自动计算；非法分类/超限拒绝）。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        event_id: [u8; 16],
        tenant_id: u32,
        site_id: u32,
        producer_id: u32,
        producer_epoch: u64,
        sequence: u64,
        occurred_at_ns: u64,
        recorded_at_ns: u64,
        schema_id: u32,
        schema_version: u16,
        classification: Classification,
        flags: u8,
        correlation_id: [u8; 16],
        causation_id: [u8; 16],
        payload: Vec<u8>,
    ) -> Result<Self, DomainDecodeError> {
        if event_id == [0u8; 16] {
            return Err(DomainDecodeError::ZeroEventId);
        }
        if payload.len() > DOMAIN_PAYLOAD_MAX {
            return Err(DomainDecodeError::PayloadTooLarge {
                len: payload.len(),
                max: DOMAIN_PAYLOAD_MAX,
            });
        }
        let payload_sha256 = Self::digest(&payload);
        Ok(Self {
            event_id,
            tenant_id,
            site_id,
            producer_id,
            producer_epoch,
            sequence,
            occurred_at_ns,
            recorded_at_ns,
            schema_id,
            schema_version,
            classification,
            flags,
            correlation_id,
            causation_id,
            payload,
            payload_sha256,
        })
    }

    fn digest(payload: &[u8]) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(b"eneros.domain-event.payload.v1");
        h.update(payload);
        h.finalize().into()
    }

    pub fn payload_sha256(&self) -> &[u8; 32] {
        &self.payload_sha256
    }

    /// 大对象引用化：payload 超 1024B 的事实以 BlobRef 引用（本类型
    /// 不携带大 payload——构造即拒绝）。
    pub fn wire_len(&self) -> usize {
        DOMAIN_HEADER_LEN + self.payload.len()
    }

    /// wire 编码（头 134B 定长 + payload）。
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.wire_len());
        out.extend_from_slice(&self.event_id);
        out.extend_from_slice(&self.tenant_id.to_le_bytes());
        out.extend_from_slice(&self.site_id.to_le_bytes());
        out.extend_from_slice(&self.producer_id.to_le_bytes());
        out.extend_from_slice(&self.producer_epoch.to_le_bytes());
        out.extend_from_slice(&self.sequence.to_le_bytes());
        out.extend_from_slice(&self.occurred_at_ns.to_le_bytes());
        out.extend_from_slice(&self.recorded_at_ns.to_le_bytes());
        out.extend_from_slice(&self.schema_id.to_le_bytes());
        out.extend_from_slice(&self.schema_version.to_le_bytes());
        out.push(self.classification.code());
        out.push(self.flags);
        out.extend_from_slice(&self.correlation_id);
        out.extend_from_slice(&self.causation_id);
        out.extend_from_slice(&(self.payload.len() as u16).to_le_bytes());
        out.extend_from_slice(&self.payload_sha256);
        out.extend_from_slice(&self.payload);
        out
    }

    /// wire 解码（先校头长/分类/计数上限再分配 payload——[06] §4.2）。
    pub fn decode(buf: &[u8]) -> Result<Self, DomainDecodeError> {
        if buf.len() < DOMAIN_HEADER_LEN {
            return Err(DomainDecodeError::Truncated);
        }
        let mut pos = 0usize;
        let mut take = |n: usize| -> Result<&[u8], DomainDecodeError> {
            if pos + n > buf.len() {
                return Err(DomainDecodeError::Truncated);
            }
            let s = &buf[pos..pos + n];
            pos += n;
            Ok(s)
        };
        let event_id = take(16)?.try_into().unwrap();
        if event_id == [0u8; 16] {
            return Err(DomainDecodeError::ZeroEventId);
        }
        let tenant_id = u32::from_le_bytes(take(4)?.try_into().unwrap());
        let site_id = u32::from_le_bytes(take(4)?.try_into().unwrap());
        let producer_id = u32::from_le_bytes(take(4)?.try_into().unwrap());
        let producer_epoch = u64::from_le_bytes(take(8)?.try_into().unwrap());
        let sequence = u64::from_le_bytes(take(8)?.try_into().unwrap());
        let occurred_at_ns = u64::from_le_bytes(take(8)?.try_into().unwrap());
        let recorded_at_ns = u64::from_le_bytes(take(8)?.try_into().unwrap());
        let schema_id = u32::from_le_bytes(take(4)?.try_into().unwrap());
        let schema_version = u16::from_le_bytes(take(2)?.try_into().unwrap());
        let classification =
            Classification::from_code(take(1)?[0]).ok_or(DomainDecodeError::BadClassification)?;
        let flags = take(1)?[0];
        let correlation_id = take(16)?.try_into().unwrap();
        let causation_id = take(16)?.try_into().unwrap();
        let payload_len = u16::from_le_bytes(take(2)?.try_into().unwrap()) as usize;
        if payload_len > DOMAIN_PAYLOAD_MAX {
            return Err(DomainDecodeError::PayloadTooLarge {
                len: payload_len,
                max: DOMAIN_PAYLOAD_MAX,
            });
        }
        let payload_sha256: [u8; 32] = take(32)?.try_into().unwrap();
        if pos + payload_len != buf.len() {
            return Err(DomainDecodeError::Truncated);
        }
        let payload = buf[pos..pos + payload_len].to_vec();
        // payload 摘要复验（篡改可检出——[06] §4.2）。
        if Self::digest(&payload) != payload_sha256 {
            return Err(DomainDecodeError::PayloadDigestMismatch);
        }
        Ok(Self {
            event_id,
            tenant_id,
            site_id,
            producer_id,
            producer_epoch,
            sequence,
            occurred_at_ns,
            recorded_at_ns,
            schema_id,
            schema_version,
            classification,
            flags,
            correlation_id,
            causation_id,
            payload,
            payload_sha256,
        })
    }
}

/// 大对象引用（[02] §3.3 定长 76B；读取必须再校验 size/digest/ACL）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobRef {
    pub store_id: u32,
    pub object_id: [u8; 16],
    pub size_bytes: u64,
    pub sha256: [u8; 32],
    pub media_type_id: u32,
    pub encryption_key_id: u32,
    pub expires_at_ns: u64,
}

impl BlobRef {
    pub fn encode(&self) -> [u8; BLOB_REF_WIRE_LEN] {
        let mut out = [0u8; BLOB_REF_WIRE_LEN];
        out[0..4].copy_from_slice(&self.store_id.to_le_bytes());
        out[4..20].copy_from_slice(&self.object_id);
        out[20..28].copy_from_slice(&self.size_bytes.to_le_bytes());
        out[28..60].copy_from_slice(&self.sha256);
        out[60..64].copy_from_slice(&self.media_type_id.to_le_bytes());
        out[64..68].copy_from_slice(&self.encryption_key_id.to_le_bytes());
        out[68..76].copy_from_slice(&self.expires_at_ns.to_le_bytes());
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Self, DomainDecodeError> {
        if buf.len() != BLOB_REF_WIRE_LEN {
            return Err(DomainDecodeError::Truncated);
        }
        Ok(Self {
            store_id: u32::from_le_bytes(buf[0..4].try_into().unwrap()),
            object_id: buf[4..20].try_into().unwrap(),
            size_bytes: u64::from_le_bytes(buf[20..28].try_into().unwrap()),
            sha256: buf[28..60].try_into().unwrap(),
            media_type_id: u32::from_le_bytes(buf[60..64].try_into().unwrap()),
            encryption_key_id: u32::from_le_bytes(buf[64..68].try_into().unwrap()),
            expires_at_ns: u64::from_le_bytes(buf[68..76].try_into().unwrap()),
        })
    }
}

/// DomainEvent/BlobRef 解码错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainDecodeError {
    Truncated,
    ZeroEventId,
    BadClassification,
    PayloadTooLarge { len: usize, max: usize },
    PayloadDigestMismatch,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    fn event() -> DomainEvent {
        DomainEvent::new(
            [1u8; 16],
            1,
            1,
            2,
            1,
            1,
            1_000,
            2_000,
            1,
            1,
            Classification::Confidential,
            0,
            [3u8; 16],
            [4u8; 16],
            alloc::vec![0xDE, 0xAD, 0xBE, 0xEF],
        )
        .unwrap()
    }

    #[test]
    fn roundtrip_and_digest_binding() {
        let e = event();
        let wire = e.encode();
        assert_eq!(wire.len(), DOMAIN_HEADER_LEN + 4);
        let d = DomainEvent::decode(&wire).unwrap();
        assert_eq!(d, e);
        // payload 篡改检出（payload SHA-256 复验）。
        let mut tampered = wire.clone();
        let n = tampered.len();
        tampered[n - 1] ^= 0xFF;
        assert_eq!(
            DomainEvent::decode(&tampered),
            Err(DomainDecodeError::PayloadDigestMismatch)
        );
    }

    #[test]
    fn oversize_and_classification_fail_closed() {
        // 超限 payload（1025B）拒绝——走 BlobRef。
        let r = DomainEvent::new(
            [1u8; 16],
            1,
            1,
            2,
            1,
            1,
            1,
            1,
            1,
            1,
            Classification::Internal,
            0,
            [0; 16],
            [0; 16],
            alloc::vec![0u8; DOMAIN_PAYLOAD_MAX + 1],
        );
        assert_eq!(
            r,
            Err(DomainDecodeError::PayloadTooLarge {
                len: 1025,
                max: 1024
            })
        );
        // 未知分类码拒绝。
        let mut wire = event().encode();
        wire[66] = 0xFE; // classification 字节（16+4+4+4+8+8+8+8+4+2=66）
        assert_eq!(
            DomainEvent::decode(&wire),
            Err(DomainDecodeError::BadClassification)
        );
    }

    #[test]
    fn blob_ref_roundtrip() {
        let b = BlobRef {
            store_id: 1,
            object_id: [2; 16],
            size_bytes: 1 << 20,
            sha256: [3; 32],
            media_type_id: 4,
            encryption_key_id: 5,
            expires_at_ns: 6,
        };
        let wire = b.encode();
        assert_eq!(wire.len(), BLOB_REF_WIRE_LEN);
        assert_eq!(BlobRef::decode(&wire).unwrap(), b);
        assert_eq!(
            BlobRef::decode(&wire[..75]),
            Err(DomainDecodeError::Truncated)
        );
    }

    #[test]
    fn size_relationships_documented() {
        // 编译期断言在模块级（const _）；此处仅做可读性登记。
        let _ = (DOMAIN_EVENT_MAX_WIRE, CHUNK_LOGICAL_CAP, BLOB_REF_WIRE_LEN);
    }
}
