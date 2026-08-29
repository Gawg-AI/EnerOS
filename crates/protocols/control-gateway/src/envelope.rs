//! Command envelope codec (CTL-003).
//!
//! A [`CommandEnvelope`] is a fixed-size, length-bounded authorization envelope
//! that wraps a single write command destined for a device point. Every write
//! that reaches a device must arrive inside a verified envelope; the control
//! gateway ([`crate::gateway::ControlGateway`]) is the only path that can
//! produce an [`crate::gateway::AuthorizedCommand`] from raw envelope bytes, and
//! it is **default-deny**: any verification failure rejects the write.
//!
//! # Wire format (172 bytes, little-endian, fixed-size, version 2)
//!
//! ```text
//!  off  len  field
//!   0    1   version            (u8, currently 2)
//!   1   16   command_id         (unique per command; replay protection)
//!  17   16   correlation_id      (traces a request→result chain)
//!  33    8   sequence           (u64, monotonic per-issuer ordering)
//!  41    4   device_id          (u32)
//!  45    4   point_id           (u32)
//!  49    1   command_tag        (0 = Single, 1 = Double)
//!  50    1   command_value      (Single: 0=Off/1=On; Double: 0/1/2/3)
//!  51    1   actor_kind         (0=Operator,1=Agent,2=Ai,3=System,4=Emergency)
//!  52   16   issuer             (subject id, fixed 16 bytes)
//!  68    8   timestamp          (u64, issue epoch-secs)
//!  76    8   expiry             (u64, expiry epoch-secs)
//!  84   16   lease_id           (CTL-006 lease this command rides on)
//! 100    8   fencing_token      (u64, monotonic; CTL-006 revocation fence)
//! 108    8   policy_version     (u64, PDP/PEP policy generation, CTL-005)
//! 116    8   config_version     (u64, config generation the issuer saw)
//! 124   16   idempotency_key    (CTL-007 dedup key)
//! 140   32   hmac               (SM3-HMAC over bytes [0..140])
//! ```
//!
//! Fixed size means the decoder never feeds untrusted data into an unbounded
//! allocator: a buffer that is not exactly [`ENVELOPE_LEN`] bytes is rejected
//! before any field is read.
//!
//! # Authorization
//!
//! The HMAC covers the 140-byte header (everything except the HMAC itself),
//! keyed by a shared secret provisioned out-of-band. Verification uses
//! constant-time comparison ([`eneros_crypto::ct_eq`]) to avoid timing
//! oracles. A missing or mismatched HMAC is a hard deny.

use eneros_crypto::{ct_eq, hmac_sm3};
// 蓝图/24 D4：`ActorKind` 下移至共享模型层（telemetry-model::command），
// re-export 保持 `crate::envelope::ActorKind` 既有导入路径。
pub use eneros_telemetry_model::command::ActorKind;
use eneros_telemetry_model::command::{CommandValue, ControlCommand, DoubleCommand, SingleCommand};

/// Total envelope length in bytes（V2 legacy 布局）。
pub const ENVELOPE_LEN: usize = 172;
/// Header length (everything before the HMAC)（V2 legacy）。
pub const HEADER_LEN: usize = 140;
/// V2 legacy envelope version.
pub const VERSION: u8 = 2;

/// V3 信封总长（61 v0.314.25：+value_tag(1) + value_bits(8)）。
pub const ENVELOPE_LEN_V3: usize = 181;
/// V3 头长（HMAC 覆盖域，含 typed value）。
pub const HEADER_LEN_V3: usize = 149;
/// V3 envelope version.
pub const VERSION_V3: u8 = 3;

/// Errors raised by the envelope codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvelopeError {
    /// Buffer is not exactly [`ENVELOPE_LEN`] bytes.
    BadLength { got: usize },
    /// Unsupported envelope version.
    BadVersion { got: u8 },
    /// Command tag/value is not a valid encoding.
    BadCommand { tag: u8, value: u8 },
    /// Actor kind byte is not a valid encoding.
    BadActor { got: u8 },
}

impl core::fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EnvelopeError::BadLength { got } => {
                write!(
                    f,
                    "envelope bad length: got {}, expected {}",
                    got, ENVELOPE_LEN
                )
            }
            EnvelopeError::BadVersion { got } => {
                write!(f, "envelope bad version: got {}, expected {}", got, VERSION)
            }
            EnvelopeError::BadCommand { tag, value } => {
                write!(f, "envelope bad command: tag={}, value={}", tag, value)
            }
            EnvelopeError::BadActor { got } => {
                write!(f, "envelope bad actor kind: {}", got)
            }
        }
    }
}

/// A command envelope. The `hmac` field is computed over the encoded header
/// ([`HEADER_LEN`] bytes). Construct with [`EnvelopeBuilder`]; decode untrusted
/// bytes with [`CommandEnvelope::decode`].
// Eq 不可用（CommandValue 含 f64）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommandEnvelope {
    pub command_id: [u8; 16],
    pub correlation_id: [u8; 16],
    pub sequence: u64,
    pub device_id: u32,
    pub point_id: u32,
    /// 类型化命令值（61 v0.314.25：V2 legacy 解码映射为 SwitchPosition）。
    pub value: CommandValue,
    pub actor_kind: ActorKind,
    pub issuer: [u8; 16],
    pub timestamp: u64,
    pub expiry: u64,
    pub lease_id: [u8; 16],
    pub fencing_token: u64,
    pub policy_version: u64,
    pub config_version: u64,
    pub idempotency_key: [u8; 16],
    pub hmac: [u8; 32],
}

/// Builder for a [`CommandEnvelope`]. All fields with safety meaning default to
/// zero/empty; the gateway treats zero `fencing_token`/`policy_version` as
/// structurally invalid (a hard deny), so production callers MUST set them via
/// the lease/policy services (CTL-005/006).
pub struct EnvelopeBuilder {
    command_id: [u8; 16],
    correlation_id: [u8; 16],
    sequence: u64,
    device_id: u32,
    point_id: u32,
    command: ControlCommand,
    value: Option<CommandValue>,
    actor_kind: ActorKind,
    issuer: [u8; 16],
    timestamp: u64,
    expiry: u64,
    lease_id: [u8; 16],
    fencing_token: u64,
    policy_version: u64,
    config_version: u64,
    idempotency_key: [u8; 16],
}

impl EnvelopeBuilder {
    /// Start a builder with the core identifying fields.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        command_id: [u8; 16],
        device_id: u32,
        point_id: u32,
        command: ControlCommand,
        timestamp: u64,
        expiry: u64,
        issuer: [u8; 16],
    ) -> Self {
        Self {
            command_id,
            correlation_id: command_id,
            sequence: 0,
            device_id,
            point_id,
            command,
            value: None,
            actor_kind: ActorKind::Operator,
            issuer,
            timestamp,
            expiry,
            lease_id: [0u8; 16],
            fencing_token: 0,
            policy_version: 0,
            config_version: 0,
            idempotency_key: [0u8; 16],
        }
    }

    pub fn correlation_id(mut self, id: [u8; 16]) -> Self {
        self.correlation_id = id;
        self
    }
    pub fn sequence(mut self, seq: u64) -> Self {
        self.sequence = seq;
        self
    }
    pub fn actor_kind(mut self, kind: ActorKind) -> Self {
        self.actor_kind = kind;
        self
    }
    /// Bind this command to a lease (CTL-006). A command with a zero
    /// `fencing_token` is structurally denied by the gateway.
    pub fn lease(mut self, lease_id: [u8; 16], fencing_token: u64) -> Self {
        self.lease_id = lease_id;
        self.fencing_token = fencing_token;
        self
    }
    pub fn policy_version(mut self, v: u64) -> Self {
        self.policy_version = v;
        self
    }
    pub fn config_version(mut self, v: u64) -> Self {
        self.config_version = v;
        self
    }
    pub fn idempotency_key(mut self, key: [u8; 16]) -> Self {
        self.idempotency_key = key;
        self
    }
    /// 类型化命令值（61 v0.314.25）。设置后覆盖 `new` 传入的 legacy
    /// ControlCommand 语义——typed value 是唯一执行数据源。
    pub fn value(mut self, value: CommandValue) -> Self {
        self.value = Some(value);
        self
    }

    /// Finalize: build a **V3** envelope; the typed value（显式 `.value()`
    /// 或由 legacy ControlCommand 派生的 SwitchPosition）进入 HMAC 覆盖域。
    pub fn build(self, key: &[u8]) -> CommandEnvelope {
        let value = self.value.unwrap_or(match self.command {
            ControlCommand::Single(SingleCommand::On) => CommandValue::SwitchPosition(1),
            ControlCommand::Single(SingleCommand::Off) => CommandValue::SwitchPosition(0),
            ControlCommand::Double(DoubleCommand::Off) => CommandValue::SwitchPosition(0),
            ControlCommand::Double(_) => CommandValue::SwitchPosition(1),
        });
        let env = CommandEnvelope {
            command_id: self.command_id,
            correlation_id: self.correlation_id,
            sequence: self.sequence,
            device_id: self.device_id,
            point_id: self.point_id,
            value,
            actor_kind: self.actor_kind,
            issuer: self.issuer,
            timestamp: self.timestamp,
            expiry: self.expiry,
            lease_id: self.lease_id,
            fencing_token: self.fencing_token,
            policy_version: self.policy_version,
            config_version: self.config_version,
            idempotency_key: self.idempotency_key,
            hmac: [0u8; 32],
        };
        let mut buf = [0u8; ENVELOPE_LEN_V3];
        env.encode_header_v3_into(&mut buf);
        let mut out = env;
        out.hmac = hmac_sm3(key, &buf[..HEADER_LEN_V3]);
        out
    }
}

impl CommandEnvelope {
    /// V3 头部编码（149B；value_tag@51、value_bits@52..60 由 HMAC 覆盖）。
    fn encode_header_v3_into(&self, buf: &mut [u8; ENVELOPE_LEN_V3]) {
        buf[0] = VERSION_V3;
        buf[1..17].copy_from_slice(&self.command_id);
        buf[17..33].copy_from_slice(&self.correlation_id);
        buf[33..41].copy_from_slice(&self.sequence.to_le_bytes());
        buf[41..45].copy_from_slice(&self.device_id.to_le_bytes());
        buf[45..49].copy_from_slice(&self.point_id.to_le_bytes());
        let (tag, value) = match self.value.legacy_command() {
            Some(ControlCommand::Single(SingleCommand::Off)) => (0u8, 0u8),
            Some(ControlCommand::Single(SingleCommand::On)) => (0u8, 1u8),
            // 连续/模拟值走 Typed tag——legacy tag 仅保留给开关位。
            _ => (2u8, 0u8),
        };
        buf[49] = tag;
        buf[50] = value;
        buf[51] = self.value.value_tag();
        buf[52..60].copy_from_slice(&self.value.value_bits());
        buf[60] = self.actor_kind as u8;
        buf[61..77].copy_from_slice(&self.issuer);
        buf[77..85].copy_from_slice(&self.timestamp.to_le_bytes());
        buf[85..93].copy_from_slice(&self.expiry.to_le_bytes());
        buf[93..109].copy_from_slice(&self.lease_id);
        buf[109..117].copy_from_slice(&self.fencing_token.to_le_bytes());
        buf[117..125].copy_from_slice(&self.policy_version.to_le_bytes());
        buf[125..133].copy_from_slice(&self.config_version.to_le_bytes());
        buf[133..149].copy_from_slice(&self.idempotency_key);
    }

    /// V3 全量编码（头 149B + HMAC 32B）。
    pub fn encode(&self) -> [u8; ENVELOPE_LEN_V3] {
        let mut buf = [0u8; ENVELOPE_LEN_V3];
        self.encode_header_v3_into(&mut buf);
        buf[149..181].copy_from_slice(&self.hmac);
        buf
    }

    /// 降级编码为 V2 legacy（61 v0.314.25 §3.6：仅明确 legacy Switch 允许；
    /// 连续值返回 None——不得把浮点解释进 172B 开关位）。
    pub fn encode_legacy_v2(&self) -> Option<[u8; ENVELOPE_LEN]> {
        let cmd = self.value.legacy_command()?;
        let mut buf = [0u8; ENVELOPE_LEN];
        buf[0] = VERSION;
        buf[1..17].copy_from_slice(&self.command_id);
        buf[17..33].copy_from_slice(&self.correlation_id);
        buf[33..41].copy_from_slice(&self.sequence.to_le_bytes());
        buf[41..45].copy_from_slice(&self.device_id.to_le_bytes());
        buf[45..49].copy_from_slice(&self.point_id.to_le_bytes());
        let (tag, value) = encode_command(&cmd);
        buf[49] = tag;
        buf[50] = value;
        buf[51] = self.actor_kind as u8;
        buf[52..68].copy_from_slice(&self.issuer);
        buf[68..76].copy_from_slice(&self.timestamp.to_le_bytes());
        buf[76..84].copy_from_slice(&self.expiry.to_le_bytes());
        buf[84..100].copy_from_slice(&self.lease_id);
        buf[100..108].copy_from_slice(&self.fencing_token.to_le_bytes());
        buf[108..116].copy_from_slice(&self.policy_version.to_le_bytes());
        buf[116..124].copy_from_slice(&self.config_version.to_le_bytes());
        buf[124..140].copy_from_slice(&self.idempotency_key);
        buf[140..172].copy_from_slice(&self.hmac);
        Some(buf)
    }

    /// 解码 + 结构校验（不验 HMAC）：181B → V3；172B → V2 legacy
    /// （映射 SwitchPosition；legacy 浮点不存在）。其余长度拒绝。
    pub fn decode(buf: &[u8]) -> Result<Self, EnvelopeError> {
        match buf.len() {
            ENVELOPE_LEN_V3 => Self::decode_v3(buf),
            ENVELOPE_LEN => Self::decode_v2_legacy(buf),
            got => Err(EnvelopeError::BadLength { got }),
        }
    }

    fn decode_v3(buf: &[u8]) -> Result<Self, EnvelopeError> {
        if buf[0] != VERSION_V3 {
            return Err(EnvelopeError::BadVersion { got: buf[0] });
        }
        let command_id: [u8; 16] = buf[1..17].try_into().unwrap();
        let correlation_id: [u8; 16] = buf[17..33].try_into().unwrap();
        let sequence = u64::from_le_bytes(buf[33..41].try_into().unwrap());
        let device_id = u32::from_le_bytes([buf[41], buf[42], buf[43], buf[44]]);
        let point_id = u32::from_le_bytes([buf[45], buf[46], buf[47], buf[48]]);
        // tag 0 = legacy Single（value 字节为开关位）；tag 2 = Typed。
        let value = match (buf[49], buf[50]) {
            (0, v) if v <= 1 => CommandValue::SwitchPosition(v),
            (2, _) => {
                let bits: [u8; 8] = buf[52..60].try_into().unwrap();
                CommandValue::from_tag_bits(buf[51], bits).ok_or(EnvelopeError::BadCommand {
                    tag: buf[49],
                    value: buf[51],
                })?
            }
            (tag, value) => return Err(EnvelopeError::BadCommand { tag, value }),
        };
        // tag 0 时 value_tag/value_bits 必须与开关位一致（防混合编码）。
        if buf[49] == 0 {
            let bits: [u8; 8] = buf[52..60].try_into().unwrap();
            match CommandValue::from_tag_bits(buf[51], bits) {
                Some(v)
                    if v.value_tag() == value.value_tag()
                        && v.value_bits() == value.value_bits() => {}
                _ => {
                    return Err(EnvelopeError::BadCommand {
                        tag: buf[49],
                        value: buf[51],
                    })
                }
            }
        }
        let actor_kind =
            ActorKind::from_byte(buf[60]).ok_or(EnvelopeError::BadActor { got: buf[60] })?;
        let issuer: [u8; 16] = buf[61..77].try_into().unwrap();
        let timestamp = u64::from_le_bytes(buf[77..85].try_into().unwrap());
        let expiry = u64::from_le_bytes(buf[85..93].try_into().unwrap());
        let lease_id: [u8; 16] = buf[93..109].try_into().unwrap();
        let fencing_token = u64::from_le_bytes(buf[109..117].try_into().unwrap());
        let policy_version = u64::from_le_bytes(buf[117..125].try_into().unwrap());
        let config_version = u64::from_le_bytes(buf[125..133].try_into().unwrap());
        let idempotency_key: [u8; 16] = buf[133..149].try_into().unwrap();
        let hmac: [u8; 32] = buf[149..181].try_into().unwrap();
        Ok(Self {
            command_id,
            correlation_id,
            sequence,
            device_id,
            point_id,
            value,
            actor_kind,
            issuer,
            timestamp,
            expiry,
            lease_id,
            fencing_token,
            policy_version,
            config_version,
            idempotency_key,
            hmac,
        })
    }

    fn decode_v2_legacy(buf: &[u8]) -> Result<Self, EnvelopeError> {
        if buf[0] != VERSION {
            return Err(EnvelopeError::BadVersion { got: buf[0] });
        }
        let command_id: [u8; 16] = buf[1..17].try_into().unwrap();
        let correlation_id: [u8; 16] = buf[17..33].try_into().unwrap();
        let sequence = u64::from_le_bytes(buf[33..41].try_into().unwrap());
        let device_id = u32::from_le_bytes([buf[41], buf[42], buf[43], buf[44]]);
        let point_id = u32::from_le_bytes([buf[45], buf[46], buf[47], buf[48]]);
        let legacy = decode_command(buf[49], buf[50])?;
        // 61 v0.314.25：V2 legacy 只有开关语义；连续浮点不存在于该布局。
        let value = match legacy {
            ControlCommand::Single(SingleCommand::On) => CommandValue::SwitchPosition(1),
            ControlCommand::Single(SingleCommand::Off) => CommandValue::SwitchPosition(0),
            ControlCommand::Double(DoubleCommand::Off) => CommandValue::SwitchPosition(0),
            ControlCommand::Double(_) => CommandValue::SwitchPosition(1),
        };
        let actor_kind =
            ActorKind::from_byte(buf[51]).ok_or(EnvelopeError::BadActor { got: buf[51] })?;
        let issuer: [u8; 16] = buf[52..68].try_into().unwrap();
        let timestamp = u64::from_le_bytes(buf[68..76].try_into().unwrap());
        let expiry = u64::from_le_bytes(buf[76..84].try_into().unwrap());
        let lease_id: [u8; 16] = buf[84..100].try_into().unwrap();
        let fencing_token = u64::from_le_bytes(buf[100..108].try_into().unwrap());
        let policy_version = u64::from_le_bytes(buf[108..116].try_into().unwrap());
        let config_version = u64::from_le_bytes(buf[116..124].try_into().unwrap());
        let idempotency_key: [u8; 16] = buf[124..140].try_into().unwrap();
        let hmac: [u8; 32] = buf[140..172].try_into().unwrap();
        Ok(Self {
            command_id,
            correlation_id,
            sequence,
            device_id,
            point_id,
            value,
            actor_kind,
            issuer,
            timestamp,
            expiry,
            lease_id,
            fencing_token,
            policy_version,
            config_version,
            idempotency_key,
            hmac,
        })
    }

    /// V3 HMAC 重算比对（HMAC 覆盖含 typed value 的 149B 头）。
    pub fn verify_hmac(&self, key: &[u8]) -> bool {
        let mut buf = [0u8; ENVELOPE_LEN_V3];
        self.encode_header_v3_into(&mut buf);
        let expected = hmac_sm3(key, &buf[..HEADER_LEN_V3]);
        ct_eq(&expected, &self.hmac)
    }

    /// 命令摘要（61 v0.314.25 §3.3）：
    /// `SHA-256("eneros.authorized-command.v2" ‖ header_v3_without_hmac)`。
    /// writer 侧从收到的信封字节重算比对——值 50→60 必然可检出。
    pub fn command_digest(&self) -> [u8; 32] {
        use sha2::{Digest as _, Sha256};
        let mut buf = [0u8; ENVELOPE_LEN_V3];
        self.encode_header_v3_into(&mut buf);
        let mut h = Sha256::new();
        h.update(b"eneros.authorized-command.v2");
        h.update(&buf[..HEADER_LEN_V3]);
        h.finalize().into()
    }
}

/// Encodes a [`ControlCommand`] into a (tag, value) byte pair.
fn encode_command(c: &ControlCommand) -> (u8, u8) {
    match c {
        ControlCommand::Single(SingleCommand::Off) => (0, 0),
        ControlCommand::Single(SingleCommand::On) => (0, 1),
        ControlCommand::Double(DoubleCommand::Off) => (1, 0),
        ControlCommand::Double(DoubleCommand::On) => (1, 1),
        ControlCommand::Double(DoubleCommand::Intermediate) => (1, 2),
        ControlCommand::Double(DoubleCommand::Bad) => (1, 3),
    }
}

/// Decodes a (tag, value) byte pair into a [`ControlCommand`].
fn decode_command(tag: u8, value: u8) -> Result<ControlCommand, EnvelopeError> {
    match (tag, value) {
        (0, 0) => Ok(ControlCommand::Single(SingleCommand::Off)),
        (0, 1) => Ok(ControlCommand::Single(SingleCommand::On)),
        (1, 0) => Ok(ControlCommand::Double(DoubleCommand::Off)),
        (1, 1) => Ok(ControlCommand::Double(DoubleCommand::On)),
        (1, 2) => Ok(ControlCommand::Double(DoubleCommand::Intermediate)),
        (1, 3) => Ok(ControlCommand::Double(DoubleCommand::Bad)),
        _ => Err(EnvelopeError::BadCommand { tag, value }),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    const KEY: &[u8] = b"test-key-for-ctl003-envelope-xx";

    fn now() -> u64 {
        1_700_000_000
    }

    fn sample_builder() -> EnvelopeBuilder {
        EnvelopeBuilder::new(
            [1u8; 16],
            42,
            7,
            ControlCommand::Single(SingleCommand::On),
            now(),
            now() + 30,
            [0xAB; 16],
        )
        .correlation_id([2u8; 16])
        .sequence(99)
        .actor_kind(ActorKind::Agent)
        .lease([3u8; 16], 7)
        .policy_version(3)
        .config_version(11)
        .idempotency_key([4u8; 16])
    }

    #[test]
    fn test_envelope_encode_decode_roundtrip() {
        let env = sample_builder().build(KEY);
        let buf = env.encode();
        assert_eq!(buf.len(), ENVELOPE_LEN_V3);
        let back = CommandEnvelope::decode(&buf).unwrap();
        assert_eq!(back, env);
    }

    #[test]
    fn test_decode_rejects_bad_length() {
        let err = CommandEnvelope::decode(&[0u8; 171]).unwrap_err();
        assert_eq!(err, EnvelopeError::BadLength { got: 171 });
        let err = CommandEnvelope::decode(&[0u8; 173]).unwrap_err();
        assert_eq!(err, EnvelopeError::BadLength { got: 173 });
    }

    #[test]
    fn test_decode_rejects_bad_version() {
        let mut buf = [0u8; ENVELOPE_LEN];
        buf[0] = 99;
        let err = CommandEnvelope::decode(&buf).unwrap_err();
        assert_eq!(err, EnvelopeError::BadVersion { got: 99 });
    }

    #[test]
    fn test_decode_rejects_bad_command() {
        let env = sample_builder().build(KEY);
        let mut buf = env.encode();
        buf[49] = 9; // bad tag
        buf[50] = 9; // bad value
        let err = CommandEnvelope::decode(&buf).unwrap_err();
        assert_eq!(err, EnvelopeError::BadCommand { tag: 9, value: 9 });
    }

    #[test]
    fn test_decode_rejects_bad_actor() {
        let env = sample_builder().build(KEY);
        let mut buf = env.encode();
        buf[60] = 250; // bad actor kind（V3 偏移：value_tag@51/bits@52 后移）
        let err = CommandEnvelope::decode(&buf).unwrap_err();
        assert_eq!(err, EnvelopeError::BadActor { got: 250 });
    }

    #[test]
    fn test_verify_hmac_accepts_genuine_envelope() {
        let env = sample_builder().build(KEY);
        assert!(env.verify_hmac(KEY));
    }

    #[test]
    fn test_verify_hmac_rejects_tampered_envelope() {
        let mut env = sample_builder().build(KEY);
        env.device_id = 999;
        assert!(!env.verify_hmac(KEY));
    }

    #[test]
    fn test_verify_hmac_rejects_tampered_lease() {
        let mut env = sample_builder().build(KEY);
        env.fencing_token = 999; // covered by HMAC
        assert!(!env.verify_hmac(KEY));
    }

    #[test]
    fn test_verify_hmac_rejects_wrong_key() {
        let env = sample_builder().build(KEY);
        assert!(!env.verify_hmac(b"wrong-key"));
    }

    #[test]
    fn test_command_tag_value_roundtrip_all_variants() {
        let variants = [
            ControlCommand::Single(SingleCommand::Off),
            ControlCommand::Single(SingleCommand::On),
            ControlCommand::Double(DoubleCommand::Off),
            ControlCommand::Double(DoubleCommand::On),
            ControlCommand::Double(DoubleCommand::Intermediate),
            ControlCommand::Double(DoubleCommand::Bad),
        ];
        for c in variants {
            let (tag, value) = encode_command(&c);
            let back = decode_command(tag, value).unwrap();
            assert_eq!(back, c);
        }
    }

    #[test]
    fn test_fixed_size_no_unbounded_alloc() {
        let ok = [0u8; ENVELOPE_LEN];
        let _ = CommandEnvelope::decode(&ok);
        let big = vec![0u8; ENVELOPE_LEN * 100];
        assert!(matches!(
            CommandEnvelope::decode(&big),
            Err(EnvelopeError::BadLength { .. })
        ));
    }

    #[test]
    fn test_builder_defaults_correlation_equals_command_id() {
        let env = EnvelopeBuilder::new(
            [5u8; 16],
            1,
            1,
            ControlCommand::Single(SingleCommand::On),
            now(),
            now() + 30,
            [0; 16],
        )
        .build(KEY);
        assert_eq!(env.correlation_id, [5u8; 16]);
        assert_eq!(env.fencing_token, 0);
    }
}
