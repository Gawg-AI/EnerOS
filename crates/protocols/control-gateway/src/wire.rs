//! 蓝图 40 WP-40.8/40.11：ControlDispatch ↔ device-agent 的版本化线上信封
//! （`ControlEnvelope`）与结构化响应帧（`OutcomeFrame`）。
//!
//! # ControlEnvelope
//!
//! - 定长 270 字节、全小端、schema 版本化（`CONTROL_SCHEMA = 1`）；
//! - 内部承载已签名 `CommandEnvelope`（172B，HMAC-SM3 签名域不变，见
//!   [`crate::envelope`]）；
//! - `payload_digest = SM3(inner_envelope ‖ power_kw LE)`——功率载荷位于
//!   内层 HMAC 域之外，由外层摘要补足完整性（篡改功率可检出）；
//! - 服务级元数据（issuer_service/issuer_epoch/issued_at/expires_at）由
//!   服务间 capability（EIPC endpoint badge）认证 + 内层信封字段一致性
//!   校验（device-agent 收到后核对 correlation/expiry/issuer 与内层一致）。
//!
//! # OutcomeFrame
//!
//! 2 字节定长响应：`code`（阶段结果）+ `class`（拒绝分类，`DenyClass`
//! 兼容码）。`APPLIED` 细分读回结果（WP-40.11 分层成功语义）：
//! 读回已验证（物理达到目标）→ `Committed`；读回不可得 → `Indeterminate`
//! （写可能已发生，不得猜成功）；读回不符 → `Failed`。

use crate::envelope::{CommandEnvelope, ENVELOPE_LEN};

/// 线上信封 schema 版本。
pub const CONTROL_SCHEMA: u16 = 1;

/// ControlEnvelope 定长（schema 2 + correlation 16 + issuer_service 16 +
/// issuer_epoch 8 + issued_at 8 + expires_at 8 + digest 32 + envelope 172 +
/// power 8）。
pub const CONTROL_ENVELOPE_LEN: usize = 270;

/// 线上信封错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireError {
    /// 长度不是 [`CONTROL_ENVELOPE_LEN`]。
    BadLength(usize),
    /// schema 版本不受支持。
    UnsupportedSchema(u16),
    /// 内部信封解码失败。
    Envelope,
    /// payload 摘要不匹配（功率被篡改）。
    PayloadDigestMismatch,
    /// 响应帧长度错误。
    BadOutcomeLength(usize),
    /// 未知响应码。
    UnknownOutcomeCode(u8),
}

impl core::fmt::Display for WireError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            WireError::BadLength(n) => write!(f, "control-envelope bad length {n}"),
            WireError::UnsupportedSchema(s) => write!(f, "control-envelope schema {s} unsupported"),
            WireError::Envelope => write!(f, "control-envelope inner envelope decode failed"),
            WireError::PayloadDigestMismatch => {
                write!(f, "control-envelope payload digest mismatch")
            }
            WireError::BadOutcomeLength(n) => write!(f, "outcome frame bad length {n}"),
            WireError::UnknownOutcomeCode(c) => write!(f, "outcome frame unknown code {c}"),
        }
    }
}

/// ControlDispatch → device-agent 的版本化投递信封（WP-40.8）。
#[derive(Debug, Clone, Copy)]
pub struct ControlEnvelope {
    /// schema 版本（`CONTROL_SCHEMA`）。
    pub schema: u16,
    /// 关联 ID（与内层信封 correlation_id 一致）。
    pub correlation_id: [u8; 16],
    /// 签发服务标识。
    pub issuer_service: [u8; 16],
    /// 签发服务 epoch（重启计数——旧 epoch 消息拒绝）。
    pub issuer_epoch: u64,
    /// 签发时刻（单调毫秒）。
    pub issued_at_ms: u64,
    /// 过期时刻（单调毫秒；与内层信封 expiry 一致）。
    pub expires_at_ms: u64,
    /// `SM3(inner_envelope ‖ power_kw LE)`——功率载荷完整性。
    pub payload_digest: [u8; 32],
    /// 已签名 CommandEnvelope（172B，HMAC 签名域）。
    pub envelope: [u8; ENVELOPE_LEN],
    /// 功率载荷（kW，正=放电/合，负=充电/分）。
    pub power_kw: f64,
}

impl ControlEnvelope {
    /// 构造并计算 payload 摘要（`SM3(envelope ‖ power_kw LE)`）。
    pub fn new(
        correlation_id: [u8; 16],
        issuer_service: [u8; 16],
        issuer_epoch: u64,
        issued_at_ms: u64,
        expires_at_ms: u64,
        envelope: [u8; ENVELOPE_LEN],
        power_kw: f64,
    ) -> Self {
        let payload_digest = payload_digest_of(&envelope, power_kw);
        Self {
            schema: CONTROL_SCHEMA,
            correlation_id,
            issuer_service,
            issuer_epoch,
            issued_at_ms,
            expires_at_ms,
            payload_digest,
            envelope,
            power_kw,
        }
    }

    /// 定长编码（270B，全小端）。
    pub fn encode(&self) -> [u8; CONTROL_ENVELOPE_LEN] {
        let mut buf = [0u8; CONTROL_ENVELOPE_LEN];
        buf[0..2].copy_from_slice(&self.schema.to_le_bytes());
        buf[2..18].copy_from_slice(&self.correlation_id);
        buf[18..34].copy_from_slice(&self.issuer_service);
        buf[34..42].copy_from_slice(&self.issuer_epoch.to_le_bytes());
        buf[42..50].copy_from_slice(&self.issued_at_ms.to_le_bytes());
        buf[50..58].copy_from_slice(&self.expires_at_ms.to_le_bytes());
        buf[58..90].copy_from_slice(&self.payload_digest);
        buf[90..90 + ENVELOPE_LEN].copy_from_slice(&self.envelope);
        buf[90 + ENVELOPE_LEN..].copy_from_slice(&self.power_kw.to_le_bytes());
        buf
    }

    /// 解码（长度/schema 校验；payload 摘要由调用方按需验证）。
    pub fn decode(buf: &[u8]) -> Result<Self, WireError> {
        if buf.len() != CONTROL_ENVELOPE_LEN {
            return Err(WireError::BadLength(buf.len()));
        }
        let schema = u16::from_le_bytes([buf[0], buf[1]]);
        if schema != CONTROL_SCHEMA {
            return Err(WireError::UnsupportedSchema(schema));
        }
        let mut correlation_id = [0u8; 16];
        correlation_id.copy_from_slice(&buf[2..18]);
        let mut issuer_service = [0u8; 16];
        issuer_service.copy_from_slice(&buf[18..34]);
        let issuer_epoch = u64::from_le_bytes(buf[34..42].try_into().expect("8B 定长切片"));
        let issued_at_ms = u64::from_le_bytes(buf[42..50].try_into().expect("8B 定长切片"));
        let expires_at_ms = u64::from_le_bytes(buf[50..58].try_into().expect("8B 定长切片"));
        let mut payload_digest = [0u8; 32];
        payload_digest.copy_from_slice(&buf[58..90]);
        let mut envelope = [0u8; ENVELOPE_LEN];
        envelope.copy_from_slice(&buf[90..90 + ENVELOPE_LEN]);
        let power_kw =
            f64::from_le_bytes(buf[90 + ENVELOPE_LEN..].try_into().expect("8B 定长切片"));
        Ok(Self {
            schema,
            correlation_id,
            issuer_service,
            issuer_epoch,
            issued_at_ms,
            expires_at_ms,
            payload_digest,
            envelope,
            power_kw,
        })
    }

    /// 校验 payload 摘要（功率载荷完整性）。
    pub fn verify_payload_digest(&self) -> bool {
        payload_digest_of(&self.envelope, self.power_kw) == self.payload_digest
    }

    /// 解码内层已签名信封。
    pub fn decode_inner(&self) -> Result<CommandEnvelope, WireError> {
        CommandEnvelope::decode(&self.envelope).map_err(|_| WireError::Envelope)
    }
}

/// `SM3(envelope ‖ power_kw LE)`。
fn payload_digest_of(envelope: &[u8; ENVELOPE_LEN], power_kw: f64) -> [u8; 32] {
    let mut buf = [0u8; ENVELOPE_LEN + 8];
    buf[..ENVELOPE_LEN].copy_from_slice(envelope);
    buf[ENVELOPE_LEN..].copy_from_slice(&power_kw.to_le_bytes());
    eneros_crypto::sm3_hash(&buf)
}

/// 响应帧结果码（WP-40.11 分层成功语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeCode {
    /// 已应用且读回已验证（物理状态达到目标容差）→ dispatch `Committed`。
    Applied,
    /// 幂等命中（同 command_id 已执行过）→ 返回原结果，不重复写。
    IdempotentHit,
    /// 策略要求 dry-run（未写设备）→ dispatch 视为未执行。
    DryRun,
    /// 拒绝（class = DenyClass 兼容码）。
    Denied,
    /// 已写但读回不可得（设备无读通道/超时）→ dispatch `Indeterminate`。
    AppliedReadbackUnavailable,
    /// 已写但读回不符（物理状态未达目标容差）→ dispatch `Failed`。
    AppliedReadbackMismatch,
    /// 执行失败（协议写失败等）。
    Failed,
}

impl OutcomeCode {
    /// 线码。
    pub const fn code(self) -> u8 {
        match self {
            OutcomeCode::Applied => 0,
            OutcomeCode::IdempotentHit => 1,
            OutcomeCode::DryRun => 2,
            OutcomeCode::Denied => 3,
            OutcomeCode::AppliedReadbackUnavailable => 4,
            OutcomeCode::AppliedReadbackMismatch => 5,
            OutcomeCode::Failed => 6,
        }
    }

    /// 线码 → 枚举（未知码 → None，fail-closed 不猜）。
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(OutcomeCode::Applied),
            1 => Some(OutcomeCode::IdempotentHit),
            2 => Some(OutcomeCode::DryRun),
            3 => Some(OutcomeCode::Denied),
            4 => Some(OutcomeCode::AppliedReadbackUnavailable),
            5 => Some(OutcomeCode::AppliedReadbackMismatch),
            6 => Some(OutcomeCode::Failed),
            _ => None,
        }
    }
}

/// 结构化响应帧（2 字节：code + class）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutcomeFrame {
    /// 结果码。
    pub code: u8,
    /// 拒绝分类（`DenyClass` 兼容码；非拒绝场景为 0）。
    pub class: u8,
}

impl OutcomeFrame {
    /// 构造。
    pub const fn new(code: OutcomeCode, class: u8) -> Self {
        Self {
            code: code.code(),
            class,
        }
    }

    /// 定长编码。
    pub fn encode(&self) -> [u8; 2] {
        [self.code, self.class]
    }

    /// 解码（长度校验；未知码失败）。
    pub fn decode(buf: &[u8]) -> Result<Self, WireError> {
        if buf.len() != 2 {
            return Err(WireError::BadOutcomeLength(buf.len()));
        }
        if OutcomeCode::from_code(buf[0]).is_none() {
            return Err(WireError::UnknownOutcomeCode(buf[0]));
        }
        Ok(Self {
            code: buf[0],
            class: buf[1],
        })
    }

    /// 结果码枚举视图。
    pub fn outcome(&self) -> Option<OutcomeCode> {
        OutcomeCode::from_code(self.code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_bytes() -> [u8; ENVELOPE_LEN] {
        // 真实签名信封（EnvelopeBuilder + HMAC 密钥）——内层解码路径可验证。
        let key = b"wire-test-key-xxxxxxxxxxxxxxxxxxxxxxxxx";
        crate::envelope::EnvelopeBuilder::new(
            [0x5A; 16],
            1,
            10,
            eneros_telemetry_model::command::ControlCommand::Single(
                eneros_telemetry_model::command::SingleCommand::On,
            ),
            1_000,
            1_300,
            [0x22; 16],
        )
        .build(key)
        .encode()
    }

    #[test]
    fn control_envelope_roundtrip() {
        let e = ControlEnvelope::new([0x11; 16], [0x22; 16], 3, 1_000, 1_300, env_bytes(), 50.0);
        let buf = e.encode();
        assert_eq!(buf.len(), CONTROL_ENVELOPE_LEN);
        let d = ControlEnvelope::decode(&buf).expect("decode");
        assert_eq!(d.schema, CONTROL_SCHEMA);
        assert_eq!(d.correlation_id, [0x11; 16]);
        assert_eq!(d.issuer_service, [0x22; 16]);
        assert_eq!(d.issuer_epoch, 3);
        assert_eq!(d.issued_at_ms, 1_000);
        assert_eq!(d.expires_at_ms, 1_300);
        assert_eq!(d.power_kw, 50.0);
        assert!(d.verify_payload_digest());
        let _inner = d.decode_inner().expect("inner");
    }

    #[test]
    fn control_envelope_rejects_tamper_and_bad_length() {
        let e = ControlEnvelope::new([0x11; 16], [0x22; 16], 3, 1_000, 1_300, env_bytes(), 50.0);
        let mut buf = e.encode();
        // 长度不符。
        assert!(matches!(
            ControlEnvelope::decode(&buf[..100]),
            Err(WireError::BadLength(100))
        ));
        // 功率篡改 → payload 摘要校验失败（HMAC 域外的功率有完整性保护）。
        let mut tampered = e;
        tampered.power_kw = 999.0;
        let buf2 = tampered.encode();
        let d2 = ControlEnvelope::decode(&buf2).expect("decode");
        assert!(!d2.verify_payload_digest(), "功率篡改必须被摘要检出");
        // schema 篡改。
        buf[0] = 99;
        assert!(matches!(
            ControlEnvelope::decode(&buf),
            Err(WireError::UnsupportedSchema(99))
        ));
    }

    #[test]
    fn outcome_frame_codes_roundtrip() {
        for code in [
            OutcomeCode::Applied,
            OutcomeCode::IdempotentHit,
            OutcomeCode::DryRun,
            OutcomeCode::Denied,
            OutcomeCode::AppliedReadbackUnavailable,
            OutcomeCode::AppliedReadbackMismatch,
            OutcomeCode::Failed,
        ] {
            let f = OutcomeFrame::new(code, 0);
            let d = OutcomeFrame::decode(&f.encode()).expect("decode");
            assert_eq!(d.outcome(), Some(code));
        }
        assert!(matches!(
            OutcomeFrame::decode(&[0xFF, 0]),
            Err(WireError::UnknownOutcomeCode(0xFF))
        ));
        assert!(matches!(
            OutcomeFrame::decode(&[0]),
            Err(WireError::BadOutcomeLength(1))
        ));
    }
}
