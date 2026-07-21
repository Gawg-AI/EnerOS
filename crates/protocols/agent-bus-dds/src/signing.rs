//! v0.78.0 消息签名信封与可插拔签名器。
//!
//! - [`SignedEnvelope`]：消息 + header + 64 字节签名
//! - [`MessageSigner`] trait：可插拔签名（默认 [`MockSigner`]，feature `sm2` 启用 [`Sm2Signer`]）
//! - [`pack_and_sign()`] / [`unpack_and_verify()`]：构造与验签信封
//! - 防重放：5 秒时间戳窗口（`now` 由调用方注入）
//!
//! # no_std 合规
//!
//! 仅使用 `alloc::*` / `core::*`。`sm2` feature 启用时引入 `eneros-crypto`（同样 no_std）。

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::codec::CodecKind;
use crate::policy::AgentId;

/// 防重放时间戳窗口（毫秒）。
pub const TIMESTAMP_WINDOW_MS: u64 = 5_000;

/// 密钥 ID（D13：与 AgentId 解耦，支持密钥轮换）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyId(pub u64);

/// 消息 ID（D8：u64 newtype，不用 Uuid）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MsgId(pub u64);

/// 签名信封 header（7 字段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvelopeHeader {
    /// 消息 ID.
    pub msg_id: MsgId,
    /// 时间戳（毫秒）.
    pub timestamp: u64,
    /// 源 Agent ID（D11：复用 v0.77.0 policy::AgentId）.
    pub source: AgentId,
    /// Topic 名.
    pub topic: String,
    /// QoS 等级.
    pub qos: u8,
    /// 编解码格式 tag.
    pub codec: CodecKind,
    /// 密钥 ID（D13）.
    pub key_id: KeyId,
}

/// 签名信封：header + payload + 64 字节签名。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedEnvelope {
    /// 信封 header.
    pub header: EnvelopeHeader,
    /// 已序列化的消息载荷（D10：由调用方序列化）.
    pub payload: Vec<u8>,
    /// 64 字节签名.
    pub signature: [u8; 64],
}

/// 签名错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignError {
    /// 编码失败.
    EncodeFailed,
    /// 未知密钥 ID.
    UnknownKey(KeyId),
    /// 时间戳过期（疑似重放）.
    StaleTimestamp,
    /// 签名失败.
    SigningFailed,
    /// 验签失败.
    VerifyFailed,
    /// Mock 签名器错误.
    MockError,
}

impl fmt::Display for SignError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EncodeFailed => write!(f, "encode failed"),
            Self::UnknownKey(id) => write!(f, "unknown key id: {}", id.0),
            Self::StaleTimestamp => write!(f, "stale timestamp (replay suspected)"),
            Self::SigningFailed => write!(f, "signing failed"),
            Self::VerifyFailed => write!(f, "verify failed"),
            Self::MockError => write!(f, "mock signer error"),
        }
    }
}

impl core::error::Error for SignError {}

/// 可插拔签名器 trait（D7：无 Send + Sync bound）。
///
/// 实现者提供签名与验签能力。默认实现 [`MockSigner`]；启用 `sm2` feature 时可用 [`Sm2Signer`]。
pub trait MessageSigner {
    /// 对 header + payload 签名，返回 64 字节签名。
    fn sign(&self, header: &EnvelopeHeader, payload: &[u8]) -> Result<[u8; 64], SignError>;
    /// 验证签名，`now` 为当前时钟（毫秒，由调用方注入，D9）。
    fn verify(
        &self,
        header: &EnvelopeHeader,
        payload: &[u8],
        sig: &[u8; 64],
        now: u64,
    ) -> Result<bool, SignError>;
}

/// 默认 Mock 签名器（纯 Rust，无依赖，D14）。
///
/// 签名算法：确定性 — 前 8 字节为 header.timestamp 大端序，第 9 字节为 payload.len() 低 8 位，
/// 字节 9..17 为 payload 字节求和校验和（使篡改可检测），其余为 0。
/// 用于测试 pack/unpack 流程，不提供任何密码学安全保证。
#[derive(Debug, Default)]
pub struct MockSigner;

impl MockSigner {
    /// 计算确定性签名（不依赖任何密钥）。
    fn compute_sig(header: &EnvelopeHeader, payload: &[u8]) -> [u8; 64] {
        let mut sig = [0u8; 64];
        sig[..8].copy_from_slice(&header.timestamp.to_be_bytes());
        sig[8] = (payload.len() & 0xFF) as u8;
        // 简单校验和：payload 字节求和（使 payload 内容篡改可检测）。
        let mut checksum = 0u64;
        for &b in payload {
            checksum = checksum.wrapping_add(b as u64);
        }
        sig[9..17].copy_from_slice(&checksum.to_be_bytes());
        sig
    }
}

impl MessageSigner for MockSigner {
    fn sign(&self, header: &EnvelopeHeader, payload: &[u8]) -> Result<[u8; 64], SignError> {
        Ok(Self::compute_sig(header, payload))
    }

    fn verify(
        &self,
        header: &EnvelopeHeader,
        payload: &[u8],
        sig: &[u8; 64],
        now: u64,
    ) -> Result<bool, SignError> {
        if now.abs_diff(header.timestamp) > TIMESTAMP_WINDOW_MS {
            return Err(SignError::StaleTimestamp);
        }
        let expected = Self::compute_sig(header, payload);
        Ok(expected == *sig)
    }
}

/// 构造签名信封（D9：now 注入；D10：payload 为已序列化 &[u8]）。
#[allow(clippy::too_many_arguments)]
pub fn pack_and_sign(
    signer: &dyn MessageSigner,
    payload: &[u8],
    source: AgentId,
    topic: &str,
    qos: u8,
    codec: CodecKind,
    key_id: KeyId,
    msg_id: MsgId,
    now: u64,
) -> Result<SignedEnvelope, SignError> {
    let header = EnvelopeHeader {
        msg_id,
        timestamp: now,
        source,
        topic: String::from(topic),
        qos,
        codec,
        key_id,
    };
    let signature = signer.sign(&header, payload)?;
    Ok(SignedEnvelope {
        header,
        payload: payload.to_vec(),
        signature,
    })
}

/// 验证签名信封（委托 signer.verify）。
pub fn unpack_and_verify(
    signer: &dyn MessageSigner,
    envelope: &SignedEnvelope,
    now: u64,
) -> Result<bool, SignError> {
    signer.verify(
        &envelope.header,
        &envelope.payload,
        &envelope.signature,
        now,
    )
}

// ============================================================
// sm2 feature-gated: Sm2Signer + KeyStore（D14）
// ============================================================

#[cfg(feature = "sm2")]
use alloc::collections::BTreeMap; // D5: 非 HashMap

#[cfg(feature = "sm2")]
use eneros_crypto::{sm2_sign, sm2_verify, CsRng, Sm2PrivateKey, Sm2PublicKey, Sm2Signature};

/// 公钥仓库（D5：BTreeMap）。
#[cfg(feature = "sm2")]
#[derive(Debug, Default)]
pub struct KeyStore {
    keys: BTreeMap<KeyId, Sm2PublicKey>,
}

#[cfg(feature = "sm2")]
impl KeyStore {
    /// 创建空仓库.
    pub fn new() -> Self {
        Self {
            keys: BTreeMap::new(),
        }
    }

    /// 插入 peer 公钥.
    pub fn insert(&mut self, id: KeyId, pk: Sm2PublicKey) {
        self.keys.insert(id, pk);
    }

    /// 查询 peer 公钥.
    pub fn get(&self, id: &KeyId) -> Option<&Sm2PublicKey> {
        self.keys.get(id)
    }

    /// 移除 peer 公钥.
    pub fn remove(&mut self, id: &KeyId) -> Option<Sm2PublicKey> {
        self.keys.remove(id)
    }
}

/// SM2 签名器（封装 eneros-crypto，D14）。
///
/// 注意：`CsRng` 未实现 `Clone`，`sign(&self)` 内部每次创建新 `CsRng::new()`
/// （固定种子，代价低 — 仅一次 SM3 哈希；生产环境应通过 `CsRng::from_seed`
/// 注入硬件 TRNG 熵源）。
#[cfg(feature = "sm2")]
pub struct Sm2Signer {
    private_key: Sm2PrivateKey,
    public_key: Sm2PublicKey,
    key_id: KeyId,
    keystore: KeyStore,
}

#[cfg(feature = "sm2")]
impl Sm2Signer {
    /// 创建 SM2 签名器.
    pub fn new(private_key: Sm2PrivateKey, public_key: Sm2PublicKey, key_id: KeyId) -> Self {
        Self {
            private_key,
            public_key,
            key_id,
            keystore: KeyStore::new(),
        }
    }

    /// 注册 peer 公钥（用于验签）.
    pub fn register_peer_key(&mut self, id: KeyId, pk: Sm2PublicKey) {
        self.keystore.insert(id, pk);
    }

    /// 构造签名 buffer：header 关键字段 + payload（D14：不预 SM3 哈希，sm2_sign 内部已含 SM3）。
    fn build_buffer(header: &EnvelopeHeader, payload: &[u8]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(64 + payload.len());
        buf.extend_from_slice(&header.timestamp.to_be_bytes());
        buf.extend_from_slice(&header.msg_id.0.to_be_bytes());
        buf.extend_from_slice(&header.source.0.to_be_bytes());
        buf.extend_from_slice(&header.key_id.0.to_be_bytes());
        buf.push(header.qos);
        buf.push(header.codec.as_u8());
        buf.extend_from_slice(header.topic.as_bytes());
        buf.extend_from_slice(payload);
        buf
    }
}

#[cfg(feature = "sm2")]
impl MessageSigner for Sm2Signer {
    fn sign(&self, header: &EnvelopeHeader, payload: &[u8]) -> Result<[u8; 64], SignError> {
        let buf = Self::build_buffer(header, payload);
        // CsRng 未实现 Clone，每次签名创建新实例（固定种子，仅一次 SM3，代价低）。
        let mut rng = CsRng::new();
        let sig: Sm2Signature = sm2_sign(&buf, &self.private_key, &self.public_key, &mut rng)
            .map_err(|_| SignError::SigningFailed)?;
        Ok(sig.to_bytes())
    }

    fn verify(
        &self,
        header: &EnvelopeHeader,
        payload: &[u8],
        sig: &[u8; 64],
        now: u64,
    ) -> Result<bool, SignError> {
        if now.abs_diff(header.timestamp) > TIMESTAMP_WINDOW_MS {
            return Err(SignError::StaleTimestamp);
        }
        let pk = self
            .keystore
            .get(&header.key_id)
            .ok_or(SignError::UnknownKey(header.key_id))?;
        let buf = Self::build_buffer(header, payload);
        let sig_obj = Sm2Signature::from_bytes(sig);
        sm2_verify(&buf, &sig_obj, pk).map_err(|_| SignError::VerifyFailed)
    }
}
