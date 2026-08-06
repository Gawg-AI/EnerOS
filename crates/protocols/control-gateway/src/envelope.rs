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
use eneros_telemetry_model::command::{ControlCommand, DoubleCommand, SingleCommand};

/// Total envelope length in bytes.
pub const ENVELOPE_LEN: usize = 172;
/// Header length (everything before the HMAC).
pub const HEADER_LEN: usize = 140;
/// Current envelope version.
pub const VERSION: u8 = 2;

/// Who issued the command. Drives policy (CTL-005) and audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ActorKind {
    Operator = 0,
    Agent = 1,
    Ai = 2,
    System = 3,
    Emergency = 4,
}

impl ActorKind {
    fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(ActorKind::Operator),
            1 => Some(ActorKind::Agent),
            2 => Some(ActorKind::Ai),
            3 => Some(ActorKind::System),
            4 => Some(ActorKind::Emergency),
            _ => None,
        }
    }
}

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandEnvelope {
    pub command_id: [u8; 16],
    pub correlation_id: [u8; 16],
    pub sequence: u64,
    pub device_id: u32,
    pub point_id: u32,
    pub command: ControlCommand,
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

    /// Finalize: compute the SM3-HMAC over the 140-byte header and return the
    /// signed envelope.
    pub fn build(self, key: &[u8]) -> CommandEnvelope {
        let env = CommandEnvelope {
            command_id: self.command_id,
            correlation_id: self.correlation_id,
            sequence: self.sequence,
            device_id: self.device_id,
            point_id: self.point_id,
            command: self.command,
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
        let mut buf = [0u8; ENVELOPE_LEN];
        env.encode_header_into(&mut buf);
        let mut out = env;
        out.hmac = hmac_sm3(key, &buf[..HEADER_LEN]);
        out
    }
}

impl CommandEnvelope {
    /// Encodes the 140-byte header (no HMAC) into `buf`.
    fn encode_header_into(&self, buf: &mut [u8; ENVELOPE_LEN]) {
        buf[0] = VERSION;
        buf[1..17].copy_from_slice(&self.command_id);
        buf[17..33].copy_from_slice(&self.correlation_id);
        buf[33..41].copy_from_slice(&self.sequence.to_le_bytes());
        buf[41..45].copy_from_slice(&self.device_id.to_le_bytes());
        buf[45..49].copy_from_slice(&self.point_id.to_le_bytes());
        let (tag, value) = encode_command(&self.command);
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
        // HMAC (buf[140..172]) is left for the caller / verify path.
    }

    /// Encodes the full envelope (header + HMAC) into a 172-byte array.
    pub fn encode(&self) -> [u8; ENVELOPE_LEN] {
        let mut buf = [0u8; ENVELOPE_LEN];
        self.encode_header_into(&mut buf);
        buf[140..172].copy_from_slice(&self.hmac);
        buf
    }

    /// Decodes and **structurally validates** a 172-byte buffer. This does NOT
    /// verify the HMAC — call [`CommandEnvelope::verify_hmac`] for that. It
    /// only rejects bad length / version / command / actor encodings.
    pub fn decode(buf: &[u8]) -> Result<Self, EnvelopeError> {
        if buf.len() != ENVELOPE_LEN {
            return Err(EnvelopeError::BadLength { got: buf.len() });
        }
        if buf[0] != VERSION {
            return Err(EnvelopeError::BadVersion { got: buf[0] });
        }
        let mut command_id = [0u8; 16];
        command_id.copy_from_slice(&buf[1..17]);
        let mut correlation_id = [0u8; 16];
        correlation_id.copy_from_slice(&buf[17..33]);
        let sequence = u64::from_le_bytes(buf[33..41].try_into().unwrap());
        let device_id = u32::from_le_bytes([buf[41], buf[42], buf[43], buf[44]]);
        let point_id = u32::from_le_bytes([buf[45], buf[46], buf[47], buf[48]]);
        let command = decode_command(buf[49], buf[50])?;
        let actor_kind =
            ActorKind::from_byte(buf[51]).ok_or(EnvelopeError::BadActor { got: buf[51] })?;
        let mut issuer = [0u8; 16];
        issuer.copy_from_slice(&buf[52..68]);
        let timestamp = u64::from_le_bytes(buf[68..76].try_into().unwrap());
        let expiry = u64::from_le_bytes(buf[76..84].try_into().unwrap());
        let mut lease_id = [0u8; 16];
        lease_id.copy_from_slice(&buf[84..100]);
        let fencing_token = u64::from_le_bytes(buf[100..108].try_into().unwrap());
        let policy_version = u64::from_le_bytes(buf[108..116].try_into().unwrap());
        let config_version = u64::from_le_bytes(buf[116..124].try_into().unwrap());
        let mut idempotency_key = [0u8; 16];
        idempotency_key.copy_from_slice(&buf[124..140]);
        let mut hmac = [0u8; 32];
        hmac.copy_from_slice(&buf[140..172]);
        Ok(Self {
            command_id,
            correlation_id,
            sequence,
            device_id,
            point_id,
            command,
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

    /// Recomputes the HMAC over the header and compares it to the stored tag
    /// in constant time. Returns `true` if the envelope is authentic.
    pub fn verify_hmac(&self, key: &[u8]) -> bool {
        let mut buf = [0u8; ENVELOPE_LEN];
        self.encode_header_into(&mut buf);
        let expected = hmac_sm3(key, &buf[..HEADER_LEN]);
        ct_eq(&expected, &self.hmac)
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
        assert_eq!(buf.len(), ENVELOPE_LEN);
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
        buf[51] = 250; // bad actor kind
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
