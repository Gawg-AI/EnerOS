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

use crate::envelope::{CommandEnvelope, ENVELOPE_LEN, ENVELOPE_LEN_V3};

/// 线上信封 schema 版本（V1 legacy：外层带 power_kw——61 v0.314.25 起退役）。
pub const CONTROL_SCHEMA: u16 = 1;
/// 线上信封 schema V2（61 v0.314.25）：typed value 已在内层信封 HMAC 域，
/// **删除外层 power_kw**——第二执行真相不复存在；digest = SM3(envelope)。
pub const CONTROL_SCHEMA_V2: u16 = 2;

/// V1 legacy 定长（270B；仅解码迁移）。
pub const CONTROL_ENVELOPE_LEN: usize = 270;
/// V2 定长（schema 2 + correlation 16 + issuer_service 16 + issuer_epoch 8 +
/// issued_at 8 + expires_at 8 + digest 32 + envelope 181）。
pub const CONTROL_ENVELOPE_LEN_V2: usize = 271;

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
    /// `SM3(envelope)`（V2）；V1 legacy 为 SM3(envelope ‖ power)。
    pub payload_digest: [u8; 32],
    /// 已签名 CommandEnvelope（V2=181B；V1 legacy=172B 左对齐零填充）。
    pub envelope: [u8; ENVELOPE_LEN_V3],
    /// 内层信封线上长度（V2=181；V1 legacy=172）。
    pub envelope_wire_len: u16,
}

impl ControlEnvelope {
    /// V1 legacy 构造已删除（61 v0.314.25：V1 仅解码迁移；生产与测试
    /// 一律 [`Self::new_v3`]——测试字节可用 [`Self::encode_v1_legacy_bytes`]）。
    /// V1 legacy 字节构造（`#[cfg(test)]`：迁移解码测试用）。
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn encode_v1_legacy_bytes(
        correlation_id: [u8; 16],
        issuer_service: [u8; 16],
        issuer_epoch: u64,
        issued_at_ms: u64,
        expires_at_ms: u64,
        envelope: &[u8; ENVELOPE_LEN],
        power_kw: f64,
    ) -> alloc::vec::Vec<u8> {
        let mut buf = alloc::vec![0u8; CONTROL_ENVELOPE_LEN];
        buf[0..2].copy_from_slice(&CONTROL_SCHEMA.to_le_bytes());
        buf[2..18].copy_from_slice(&correlation_id);
        buf[18..34].copy_from_slice(&issuer_service);
        buf[34..42].copy_from_slice(&issuer_epoch.to_le_bytes());
        buf[42..50].copy_from_slice(&issued_at_ms.to_le_bytes());
        buf[50..58].copy_from_slice(&expires_at_ms.to_le_bytes());
        buf[58..90].copy_from_slice(&payload_digest_of(envelope, power_kw));
        buf[90..90 + ENVELOPE_LEN].copy_from_slice(envelope);
        buf[90 + ENVELOPE_LEN..].copy_from_slice(&power_kw.to_le_bytes());
        buf
    }

    /// V2 构造（61 v0.314.25）：无外层 power——typed value 在内层 HMAC 域。
    #[allow(clippy::too_many_arguments)]
    pub fn new_v3(
        correlation_id: [u8; 16],
        issuer_service: [u8; 16],
        issuer_epoch: u64,
        issued_at_ms: u64,
        expires_at_ms: u64,
        envelope: &[u8; ENVELOPE_LEN_V3],
    ) -> Self {
        let payload_digest = eneros_crypto::sm3_hash(envelope);
        Self {
            schema: CONTROL_SCHEMA_V2,
            correlation_id,
            issuer_service,
            issuer_epoch,
            issued_at_ms,
            expires_at_ms,
            payload_digest,
            envelope: *envelope,
            envelope_wire_len: ENVELOPE_LEN_V3 as u16,
        }
    }

    /// 定长编码（V2=271B；V1 无编码路径——仅解码迁移）。
    pub fn encode(&self) -> [u8; CONTROL_ENVELOPE_LEN_V2] {
        debug_assert_eq!(self.schema, CONTROL_SCHEMA_V2);
        let mut buf = [0u8; CONTROL_ENVELOPE_LEN_V2];
        buf[0..2].copy_from_slice(&self.schema.to_le_bytes());
        buf[2..18].copy_from_slice(&self.correlation_id);
        buf[18..34].copy_from_slice(&self.issuer_service);
        buf[34..42].copy_from_slice(&self.issuer_epoch.to_le_bytes());
        buf[42..50].copy_from_slice(&self.issued_at_ms.to_le_bytes());
        buf[50..58].copy_from_slice(&self.expires_at_ms.to_le_bytes());
        buf[58..90].copy_from_slice(&self.payload_digest);
        buf[90..].copy_from_slice(&self.envelope);
        buf
    }

    /// 解码：V2（271B）或 V1 legacy（270B，迁移读）。
    pub fn decode(buf: &[u8]) -> Result<Self, WireError> {
        match buf.len() {
            CONTROL_ENVELOPE_LEN_V2 => Self::decode_v2(buf),
            CONTROL_ENVELOPE_LEN => Self::decode_v1_legacy(buf),
            got => Err(WireError::BadLength(got)),
        }
    }

    fn decode_v2(buf: &[u8]) -> Result<Self, WireError> {
        let schema = u16::from_le_bytes([buf[0], buf[1]]);
        if schema != CONTROL_SCHEMA_V2 {
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
        let mut envelope = [0u8; ENVELOPE_LEN_V3];
        envelope.copy_from_slice(&buf[90..90 + ENVELOPE_LEN_V3]);
        Ok(Self {
            schema,
            correlation_id,
            issuer_service,
            issuer_epoch,
            issued_at_ms,
            expires_at_ms,
            payload_digest,
            envelope,
            envelope_wire_len: ENVELOPE_LEN_V3 as u16,
        })
    }

    fn decode_v1_legacy(buf: &[u8]) -> Result<Self, WireError> {
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
        let legacy: [u8; ENVELOPE_LEN] = buf[90..90 + ENVELOPE_LEN].try_into().expect("定长切片");
        let mut envelope = [0u8; ENVELOPE_LEN_V3];
        envelope[..ENVELOPE_LEN].copy_from_slice(&legacy);
        Ok(Self {
            schema,
            correlation_id,
            issuer_service,
            issuer_epoch,
            issued_at_ms,
            expires_at_ms,
            payload_digest,
            envelope,
            envelope_wire_len: ENVELOPE_LEN as u16,
        })
    }

    /// 是否携带 V1 遗留 power 字段（调用方对 legacy 必须按迁移规则处置）。
    pub fn has_power_field(&self) -> bool {
        self.schema == CONTROL_SCHEMA
    }

    /// 校验 payload 摘要：V2 = SM3(envelope[0..181])；V1 = SM3(envelope‖power)
    ///（legacy power 已不可从本结构读出，V1 摘要校验在解码侧由调用方
    /// 以原始字节执行——生产路径统一 V2）。
    pub fn verify_payload_digest(&self) -> bool {
        if self.schema == CONTROL_SCHEMA_V2 {
            eneros_crypto::sm3_hash(&self.envelope) == self.payload_digest
        } else {
            // V1 legacy：外层摘要覆盖 172B 信封 + power；power 已删除，
            // 无法重算——返回 false 迫使调用方走迁移判定。
            false
        }
    }

    /// 解码内层已签名信封（按线上长度切片——V1 legacy 172B 亦可解）。
    pub fn decode_inner(&self) -> Result<CommandEnvelope, WireError> {
        let n = self.envelope_wire_len as usize;
        CommandEnvelope::decode(&self.envelope[..n]).map_err(|_| WireError::Envelope)
    }
}

/// `SM3(envelope ‖ power_kw LE)`（V1 legacy 字节构造专用）。
#[cfg(test)]
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

    fn env_bytes() -> [u8; ENVELOPE_LEN_V3] {
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
        .value(eneros_telemetry_model::command::CommandValue::ActivePowerKw(50.0))
        .build(key)
        .encode()
    }

    #[test]
    fn control_envelope_roundtrip() {
        let e = ControlEnvelope::new_v3([0x11; 16], [0x22; 16], 3, 1_000, 1_300, &env_bytes());
        let buf = e.encode();
        assert_eq!(buf.len(), CONTROL_ENVELOPE_LEN_V2);
        let d = ControlEnvelope::decode(&buf).expect("decode");
        assert_eq!(d.schema, CONTROL_SCHEMA_V2);
        assert_eq!(d.correlation_id, [0x11; 16]);
        assert_eq!(d.issuer_service, [0x22; 16]);
        assert_eq!(d.issuer_epoch, 3);
        assert_eq!(d.issued_at_ms, 1_000);
        assert_eq!(d.expires_at_ms, 1_300);
        assert!(!d.has_power_field(), "V2 无外层 power");
        assert!(d.verify_payload_digest());
        let inner = d.decode_inner().expect("inner");
        assert_eq!(
            inner.value,
            eneros_telemetry_model::command::CommandValue::ActivePowerKw(50.0)
        );
    }

    #[test]
    fn control_envelope_v1_legacy_decode_only() {
        // V1（270B）仅解码迁移：has_power_field=true，摘要校验显式不承诺。
        let legacy = ControlEnvelope::encode_v1_legacy_bytes(
            [0x11; 16],
            [0x22; 16],
            3,
            1_000,
            1_300,
            &{
                // 172B 任意字节（legacy 解码走结构校验，此处仅长度/domain）。
                let mut b = [0u8; ENVELOPE_LEN];
                b[0] = crate::envelope::VERSION;
                b
            },
            50.0,
        );
        assert_eq!(legacy.len(), CONTROL_ENVELOPE_LEN);
        let d = ControlEnvelope::decode(&legacy).expect("V1 解码");
        assert!(d.has_power_field());
        assert_eq!(d.envelope_wire_len as usize, ENVELOPE_LEN);
        assert!(!d.verify_payload_digest(), "V1 摘要不可重算（power 已删）");
    }

    #[test]
    fn control_envelope_rejects_tamper_and_bad_length() {
        let e = ControlEnvelope::new_v3([0x11; 16], [0x22; 16], 3, 1_000, 1_300, &env_bytes());
        let mut buf = e.encode();
        // 长度不符。
        assert!(matches!(
            ControlEnvelope::decode(&buf[..100]),
            Err(WireError::BadLength(100))
        ));
        // 内层信封值位篡改 → payload 摘要校验失败（61 v0.314.25：值在
        // digest+HMAC 双域）。
        let mut tampered = e;
        tampered.envelope[52] ^= 0xFF;
        let buf2 = tampered.encode();
        let d2 = ControlEnvelope::decode(&buf2).expect("decode");
        assert!(!d2.verify_payload_digest(), "值篡改必须被摘要检出");
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
