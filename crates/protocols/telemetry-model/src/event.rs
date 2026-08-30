//! 蓝图 61 [02] §3.1：`ObservationEvent` — 控制热路径定长观测（不可变事实）。
//!
//! 与 [`crate::telemetry::Telemetry`]/`DataPoint`（点定义与当前状态模型）的
//! 分工：本类型是**不可变观测事实**，回答唯一不变量——谁（source）、在哪个
//! 租户/站点、哪个 epoch/sequence、何时、以什么 schema/单位/映射、什么质量
//! 与完整性产生。它不替换配置职责，只承载事实。
//!
//! # V1 wire（固定 128B，小端）
//!
//! 布局逐字段冻结（[02] §3.1 表）；`reserved` 必须全零；字符串/Vec/Rust
//! enum 不上 wire（`value_tag`/`integrity_tag` 为数字码）。
//!
//! # event_id
//!
//! `first_16(SHA-256("eneros.observation.v1" || LE(身份+时间+值字段)))`，
//! `ingest_time_ns` 与 `reserved` 不参与——同一源事件重放得到同一 ID（去重
//! 键）。字段集合或域分隔串变更必须提升 schema 版本。
//!
//! # no_std
//!
//! 纯 `core` 数据模型 + `sha2`（no_std 兼容）。

use sha2::{Digest, Sha256};

/// wire magic：`OBSV`。
pub const OBSERVATION_MAGIC: [u8; 4] = *b"OBSV";
/// V1 wire 版本号。
pub const OBSERVATION_WIRE_V1: u16 = 1;
/// V1 wire 固定长度。
pub const OBSERVATION_WIRE_LEN: usize = 128;

/// event_id 域分隔串（[02] §3.1 冻结；变更须升 schema 版本）。
pub const EVENT_ID_DOMAIN: &[u8] = b"eneros.observation.v1";

/// 值标签（数字码，不直接传 Rust enum）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueTag {
    /// IEEE-754 双精度（`value_bits` = f64 LE）。
    F64,
    /// 有符号 64 位整数。
    I64,
    /// 无符号 64 位整数。
    U64,
    /// 布尔（`value_bits[0]` = 0/1，其余清零）。
    Bool,
    /// 枚举码（`value_bits` = u64 LE）。
    Enum,
    /// 定长 16B 原始值（如 16 字节状态块）。
    Bytes16,
}

impl ValueTag {
    /// wire 数字码。
    pub const fn code(self) -> u8 {
        match self {
            ValueTag::F64 => 0,
            ValueTag::I64 => 1,
            ValueTag::U64 => 2,
            ValueTag::Bool => 3,
            ValueTag::Enum => 4,
            ValueTag::Bytes16 => 5,
        }
    }

    /// 数字码 → 标签；未知码 → `None`（decoder 拒绝）。
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(ValueTag::F64),
            1 => Some(ValueTag::I64),
            2 => Some(ValueTag::U64),
            3 => Some(ValueTag::Bool),
            4 => Some(ValueTag::Enum),
            5 => Some(ValueTag::Bytes16),
            _ => None,
        }
    }
}

/// 完整性标签（数字码；[02] §3.1 未验证/认证/签名/本地可信边）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrityTag {
    /// 未验证（不得进控制快照，除非 descriptor 明确豁免——当前无豁免）。
    Unverified,
    /// 已认证（HMAC/会话认证）。
    Authenticated,
    /// 已签名。
    Signed,
    /// 本地可信边（同进程内直接采集）。
    LocalTrustedEdge,
}

impl IntegrityTag {
    /// wire 数字码。
    pub const fn code(self) -> u8 {
        match self {
            IntegrityTag::Unverified => 0,
            IntegrityTag::Authenticated => 1,
            IntegrityTag::Signed => 2,
            IntegrityTag::LocalTrustedEdge => 3,
        }
    }

    /// 数字码 → 标签；未知码 → `None`。
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(IntegrityTag::Unverified),
            1 => Some(IntegrityTag::Authenticated),
            2 => Some(IntegrityTag::Signed),
            3 => Some(IntegrityTag::LocalTrustedEdge),
            _ => None,
        }
    }
}

/// 品质位（`quality_bits` u16 布局，[02] §3.1：valid/invalid/substituted/
/// blocked/overflow/old/uncertain）。
pub mod quality_bits {
    /// 有效。
    pub const VALID: u16 = 1 << 0;
    /// 无效。
    pub const INVALID: u16 = 1 << 1;
    /// 替代值（补采/估算）。
    pub const SUBSTITUTED: u16 = 1 << 2;
    /// 被封锁。
    pub const BLOCKED: u16 = 1 << 3;
    /// 溢出。
    pub const OVERFLOW: u16 = 1 << 4;
    /// 过时。
    pub const OLD: u16 = 1 << 5;
    /// 不确定。
    pub const UNCERTAIN: u16 = 1 << 6;

    /// 是否满足控制热路径最低质量：valid 且无 invalid/blocked/overflow。
    pub const fn is_control_eligible(bits: u16) -> bool {
        bits & VALID != 0 && bits & (INVALID | BLOCKED | OVERFLOW) == 0
    }
}

/// 事件标志位（`flags` u16：缺时刻、补采、重放、人工、仿真、测试等）。
pub mod event_flags {
    /// 事件时刻未知（`event_time_ns == 0` 时必须置位）。
    pub const TIME_MISSING: u16 = 1 << 0;
    /// 补采（非实时产生）。
    pub const BACKFILL: u16 = 1 << 1;
    /// 重放（有意重投，event_id 相同）。
    pub const REPLAY: u16 = 1 << 2;
    /// 人工录入。
    pub const MANUAL: u16 = 1 << 3;
    /// 仿真源。
    pub const SIMULATED: u16 = 1 << 4;
    /// 测试设施（不参与生产控制）。
    pub const TEST: u16 = 1 << 5;
}

/// 观测值（内存形态；wire 上是 `value_tag` + 16B `value_bits`）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ObservationValue {
    /// 双精度浮点（必须 finite——NaN/Inf 在接入门拒绝）。
    F64(f64),
    /// 有符号 64 位。
    I64(i64),
    /// 无符号 64 位。
    U64(u64),
    /// 布尔。
    Bool(bool),
    /// 枚举码。
    Enum(u64),
    /// 定长 16B 原始值。
    Bytes16([u8; 16]),
}

impl ObservationValue {
    /// → (tag, value_bits)。Bool 编码 1B，其余清零；Bytes16 原样。
    pub fn encode(self) -> (ValueTag, [u8; 16]) {
        let mut bits = [0u8; 16];
        match self {
            ObservationValue::F64(v) => {
                bits[..8].copy_from_slice(&v.to_le_bytes());
                (ValueTag::F64, bits)
            }
            ObservationValue::I64(v) => {
                bits[..8].copy_from_slice(&v.to_le_bytes());
                (ValueTag::I64, bits)
            }
            ObservationValue::U64(v) => {
                bits[..8].copy_from_slice(&v.to_le_bytes());
                (ValueTag::U64, bits)
            }
            ObservationValue::Bool(b) => {
                bits[0] = u8::from(b);
                (ValueTag::Bool, bits)
            }
            ObservationValue::Enum(v) => {
                bits[..8].copy_from_slice(&v.to_le_bytes());
                (ValueTag::Enum, bits)
            }
            ObservationValue::Bytes16(b) => (ValueTag::Bytes16, b),
        }
    }

    /// (tag, value_bits) → 值；标签/位形不匹配 → `None`（Bool 高位非零、
    /// F64 非 finite 均拒绝——[02] §3.1 浮点必须 finite）。
    pub fn decode(tag: ValueTag, bits: &[u8; 16]) -> Option<Self> {
        match tag {
            ValueTag::F64 => {
                let v = f64::from_le_bytes(bits[..8].try_into().ok()?);
                if !v.is_finite() {
                    return None;
                }
                Some(ObservationValue::F64(v))
            }
            ValueTag::I64 => Some(ObservationValue::I64(i64::from_le_bytes(
                bits[..8].try_into().ok()?,
            ))),
            ValueTag::U64 => Some(ObservationValue::U64(u64::from_le_bytes(
                bits[..8].try_into().ok()?,
            ))),
            ValueTag::Bool => {
                if bits[0] > 1 || bits[1..].iter().any(|&b| b != 0) {
                    return None;
                }
                Some(ObservationValue::Bool(bits[0] == 1))
            }
            ValueTag::Enum => Some(ObservationValue::Enum(u64::from_le_bytes(
                bits[..8].try_into().ok()?,
            ))),
            ValueTag::Bytes16 => Some(ObservationValue::Bytes16(*bits)),
        }
    }
}

/// wire 解码错误（结构化；decoder 先校长度/版本再读字段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventDecodeError {
    /// 长度不是 128。
    BadLength,
    /// magic 不是 `OBSV`。
    BadMagic,
    /// wire 版本不是 V1。
    BadVersion,
    /// header_len 不是 128。
    BadHeaderLen,
    /// reserved 非零。
    ReservedNonZero,
    /// 未知 value_tag。
    UnknownValueTag(u8),
    /// 未知 integrity_tag。
    UnknownIntegrityTag(u8),
    /// 值位形非法（Bool 高位非零 / F64 非 finite）。
    BadValueBits,
}

/// 不可变观测事实（字段与 V1 wire 一一对应；`ingest_time_ns`/`reserved`
/// 不参与 event_id）。
///
/// 构造纪律：adapter 内部只能产生 [`super::event::ObservationEvent`]（经
/// `ingestion-core::SourceRegistry::verify`），不允许 `RawObservation` 跨出
/// adapter。字段公开便于 wire 投影，但**合法性在门上校验**（`validate`，
/// 由 SourceRegistry 与 decoder 强制）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObservationEvent {
    /// 租户域。
    pub tenant_id: u32,
    /// 站点域。
    pub site_id: u32,
    /// 注册来源（SourceRegistry 分配，不由 binary 硬编码）。
    pub source_id: u32,
    /// 来源每次可信重启递增；`0` 只允许测试（flags 须含 TEST）。
    pub source_epoch: u64,
    /// 同 epoch 内严格单调。
    pub sequence: u64,
    /// 设备测量时刻；未知置 0 且 flags 含 TIME_MISSING。
    pub event_time_ns: u64,
    /// 接收时刻（不参与 event_id）。
    pub ingest_time_ns: u64,
    /// 全局资产 ID；0 = 未绑定，不得进控制快照。
    pub asset_id: u64,
    /// 站内设备短 ID（保留 DeviceId=u16）。
    pub device_id: u16,
    /// `UnitCatalog` 单位码；0 = 未知（不得进控制快照）。
    pub unit_code: u16,
    /// 站内点位 ID（PointId=u32）。
    pub point_id: u32,
    /// 注册 schema。
    pub schema_id: u32,
    /// schema 版本。
    pub schema_version: u16,
    /// 品质位（[`quality_bits`] 布局）。
    pub quality_bits: u16,
    /// 点表/映射单调世代。
    pub mapping_version: u64,
    /// 观测值。
    pub value: ObservationValue,
    /// 完整性标签。
    pub integrity: IntegrityTag,
    /// 时钟域注册号。
    pub clock_id: u16,
    /// 事件标志（[`event_flags`] 布局）。
    pub flags: u16,
}

impl ObservationEvent {
    /// 门上校验（SourceRegistry 与 decoder 共用）：
    ///
    /// 1. 浮点必须 finite；
    /// 2. `event_time_ns == 0` 必须 flags 含 TIME_MISSING，非 0 不得含；
    /// 3. `source_epoch == 0` 只允许测试（flags 含 TEST）；
    /// 4. 未知单位码（非 0 且不在 UnitCatalog）拒绝。
    pub fn validate(&self) -> Result<(), EventDecodeError> {
        if let ObservationValue::F64(v) = self.value {
            if !v.is_finite() {
                return Err(EventDecodeError::BadValueBits);
            }
        }
        let time_missing = self.flags & event_flags::TIME_MISSING != 0;
        if (self.event_time_ns == 0) != time_missing {
            return Err(EventDecodeError::BadValueBits);
        }
        if self.source_epoch == 0 && self.flags & event_flags::TEST == 0 {
            return Err(EventDecodeError::BadValueBits);
        }
        if self.unit_code != 0 && crate::unit::unit_catalog::from_code(self.unit_code).is_none() {
            return Err(EventDecodeError::BadValueBits);
        }
        Ok(())
    }

    /// event_id：`first_16(SHA-256(EVENT_ID_DOMAIN || LE(字段)))`。
    ///
    /// 字段顺序按 [02] §3.1 冻结：tenant, site, source, epoch, sequence,
    /// event_time, asset, device, unit, point, schema, schema_version,
    /// quality_bits, mapping_version, value_tag, value_bits, integrity_tag,
    /// clock_id, flags。`ingest_time_ns` 与 reserved 不参与（重放同 ID）。
    pub fn event_id(&self) -> [u8; 16] {
        let (tag, bits) = self.value.encode();
        let mut h = Sha256::new();
        h.update(EVENT_ID_DOMAIN);
        for f in [self.tenant_id, self.site_id, self.source_id] {
            h.update(f.to_le_bytes());
        }
        h.update(self.source_epoch.to_le_bytes());
        h.update(self.sequence.to_le_bytes());
        h.update(self.event_time_ns.to_le_bytes());
        h.update(self.asset_id.to_le_bytes());
        h.update(self.device_id.to_le_bytes());
        h.update(self.unit_code.to_le_bytes());
        h.update(self.point_id.to_le_bytes());
        h.update(self.schema_id.to_le_bytes());
        h.update(self.schema_version.to_le_bytes());
        h.update(self.quality_bits.to_le_bytes());
        h.update(self.mapping_version.to_le_bytes());
        h.update([tag.code()]);
        h.update(bits);
        h.update([self.integrity.code()]);
        h.update(self.clock_id.to_le_bytes());
        h.update(self.flags.to_le_bytes());
        let out = h.finalize();
        let mut id = [0u8; 16];
        id.copy_from_slice(&out[..16]);
        id
    }

    /// → V1 wire（128B，小端；reserved 清零）。
    pub fn encode_v1(&self) -> [u8; OBSERVATION_WIRE_LEN] {
        let (tag, bits) = self.value.encode();
        let mut out = [0u8; OBSERVATION_WIRE_LEN];
        out[0..4].copy_from_slice(&OBSERVATION_MAGIC);
        out[4..6].copy_from_slice(&OBSERVATION_WIRE_V1.to_le_bytes());
        out[6..8].copy_from_slice(&(OBSERVATION_WIRE_LEN as u16).to_le_bytes());
        out[8..12].copy_from_slice(&self.tenant_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.site_id.to_le_bytes());
        out[16..20].copy_from_slice(&self.source_id.to_le_bytes());
        out[20..28].copy_from_slice(&self.source_epoch.to_le_bytes());
        out[28..36].copy_from_slice(&self.sequence.to_le_bytes());
        out[36..44].copy_from_slice(&self.event_time_ns.to_le_bytes());
        out[44..52].copy_from_slice(&self.ingest_time_ns.to_le_bytes());
        out[52..60].copy_from_slice(&self.asset_id.to_le_bytes());
        out[60..62].copy_from_slice(&self.device_id.to_le_bytes());
        out[62..64].copy_from_slice(&self.unit_code.to_le_bytes());
        out[64..68].copy_from_slice(&self.point_id.to_le_bytes());
        out[68..72].copy_from_slice(&self.schema_id.to_le_bytes());
        out[72..74].copy_from_slice(&self.schema_version.to_le_bytes());
        out[74..76].copy_from_slice(&self.quality_bits.to_le_bytes());
        out[76..84].copy_from_slice(&self.mapping_version.to_le_bytes());
        out[84..100].copy_from_slice(&self.event_id());
        out[100..116].copy_from_slice(&bits);
        out[116] = tag.code();
        out[117] = self.integrity.code();
        out[118..120].copy_from_slice(&self.clock_id.to_le_bytes());
        out[120..122].copy_from_slice(&self.flags.to_le_bytes());
        out[122..128].copy_from_slice(&[0u8; 6]); // reserved
        out
    }

    /// V1 wire → 事件；先校长度/版本/上限再读字段（[06] §4.2 decoder
    /// 纪律在 V1 的投影：reserved 非零、未知 tag、非法位形全部拒绝）。
    pub fn decode_v1(buf: &[u8]) -> Result<Self, EventDecodeError> {
        if buf.len() != OBSERVATION_WIRE_LEN {
            return Err(EventDecodeError::BadLength);
        }
        if buf[0..4] != OBSERVATION_MAGIC {
            return Err(EventDecodeError::BadMagic);
        }
        if u16::from_le_bytes(buf[4..6].try_into().unwrap()) != OBSERVATION_WIRE_V1 {
            return Err(EventDecodeError::BadVersion);
        }
        if u16::from_le_bytes(buf[6..8].try_into().unwrap()) != OBSERVATION_WIRE_LEN as u16 {
            return Err(EventDecodeError::BadHeaderLen);
        }
        if buf[122..128].iter().any(|&b| b != 0) {
            return Err(EventDecodeError::ReservedNonZero);
        }
        let tag_code = buf[116];
        let tag =
            ValueTag::from_code(tag_code).ok_or(EventDecodeError::UnknownValueTag(tag_code))?;
        let integrity_code = buf[117];
        let integrity = IntegrityTag::from_code(integrity_code)
            .ok_or(EventDecodeError::UnknownIntegrityTag(integrity_code))?;
        let value = ObservationValue::decode(tag, &buf[100..116].try_into().unwrap())
            .ok_or(EventDecodeError::BadValueBits)?;
        let ev = Self {
            tenant_id: u32::from_le_bytes(buf[8..12].try_into().unwrap()),
            site_id: u32::from_le_bytes(buf[12..16].try_into().unwrap()),
            source_id: u32::from_le_bytes(buf[16..20].try_into().unwrap()),
            source_epoch: u64::from_le_bytes(buf[20..28].try_into().unwrap()),
            sequence: u64::from_le_bytes(buf[28..36].try_into().unwrap()),
            event_time_ns: u64::from_le_bytes(buf[36..44].try_into().unwrap()),
            ingest_time_ns: u64::from_le_bytes(buf[44..52].try_into().unwrap()),
            asset_id: u64::from_le_bytes(buf[52..60].try_into().unwrap()),
            device_id: u16::from_le_bytes(buf[60..62].try_into().unwrap()),
            unit_code: u16::from_le_bytes(buf[62..64].try_into().unwrap()),
            point_id: u32::from_le_bytes(buf[64..68].try_into().unwrap()),
            schema_id: u32::from_le_bytes(buf[68..72].try_into().unwrap()),
            schema_version: u16::from_le_bytes(buf[72..74].try_into().unwrap()),
            quality_bits: u16::from_le_bytes(buf[74..76].try_into().unwrap()),
            mapping_version: u64::from_le_bytes(buf[76..84].try_into().unwrap()),
            value,
            integrity,
            clock_id: u16::from_le_bytes(buf[118..120].try_into().unwrap()),
            flags: u16::from_le_bytes(buf[120..122].try_into().unwrap()),
        };
        ev.validate()?;
        Ok(ev)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    /// 合法测试事件（epoch=0 须 TEST flag；此处用 epoch=1 生产形态）。
    fn sample() -> ObservationEvent {
        use crate::unit::{unit_catalog, PhysicalUnit};
        ObservationEvent {
            tenant_id: 1,
            site_id: 1,
            source_id: 2,
            source_epoch: 1,
            sequence: 42,
            event_time_ns: 1_000,
            ingest_time_ns: 2_000,
            asset_id: 0xA,
            device_id: 1,
            unit_code: unit_catalog::code_of(PhysicalUnit::Kilowatts),
            point_id: 10,
            schema_id: 1,
            schema_version: 1,
            quality_bits: quality_bits::VALID,
            mapping_version: 1,
            value: ObservationValue::F64(50.0),
            integrity: IntegrityTag::LocalTrustedEdge,
            clock_id: 1,
            flags: 0,
        }
    }

    #[test]
    fn wire_roundtrip() {
        let ev = sample();
        let wire = ev.encode_v1();
        assert_eq!(wire.len(), 128);
        let dec = ObservationEvent::decode_v1(&wire).expect("decode");
        assert_eq!(dec, ev);
    }

    #[test]
    fn event_id_is_deterministic_and_ignores_ingest_time() {
        let a = sample();
        let mut b = sample();
        assert_eq!(a.event_id(), b.event_id());
        // ingest_time 不参与：重放同 ID。
        b.ingest_time_ns += 999;
        assert_eq!(a.event_id(), b.event_id());
        // 身份字段变化 → ID 变化。
        b.sequence += 1;
        assert_ne!(a.event_id(), b.event_id());
        // 值变化 → ID 变化。
        let mut c = sample();
        c.value = ObservationValue::F64(51.0);
        assert_ne!(a.event_id(), c.event_id());
    }

    #[test]
    fn wire_mutations_rejected() {
        let wire = sample().encode_v1();
        // reserved 非零。
        let mut w = wire;
        w[122] = 1;
        assert_eq!(
            ObservationEvent::decode_v1(&w),
            Err(EventDecodeError::ReservedNonZero)
        );
        // magic 错。
        let mut w = wire;
        w[0] = b'X';
        assert_eq!(
            ObservationEvent::decode_v1(&w),
            Err(EventDecodeError::BadMagic)
        );
        // 版本错。
        let mut w = wire;
        w[4] = 9;
        assert_eq!(
            ObservationEvent::decode_v1(&w),
            Err(EventDecodeError::BadVersion)
        );
        // 未知 value_tag。
        let mut w = wire;
        w[116] = 0xFE;
        assert_eq!(
            ObservationEvent::decode_v1(&w),
            Err(EventDecodeError::UnknownValueTag(0xFE))
        );
        // 截断。
        assert_eq!(
            ObservationEvent::decode_v1(&wire[..64]),
            Err(EventDecodeError::BadLength)
        );
    }

    #[test]
    fn nan_inf_rejected_at_door() {
        let mut ev = sample();
        ev.value = ObservationValue::F64(f64::NAN);
        assert_eq!(ev.validate(), Err(EventDecodeError::BadValueBits));
        let wire = ev.encode_v1();
        assert_eq!(
            ObservationEvent::decode_v1(&wire),
            Err(EventDecodeError::BadValueBits)
        );
        let mut ev = sample();
        ev.value = ObservationValue::F64(f64::INFINITY);
        assert!(ev.validate().is_err());
    }

    #[test]
    fn epoch_zero_requires_test_flag() {
        let mut ev = sample();
        ev.source_epoch = 0;
        assert_eq!(ev.validate(), Err(EventDecodeError::BadValueBits));
        ev.flags |= event_flags::TEST;
        assert!(ev.validate().is_ok());
        assert!(ObservationEvent::decode_v1(&ev.encode_v1()).is_ok());
    }

    #[test]
    fn event_time_zero_requires_flag() {
        let mut ev = sample();
        ev.event_time_ns = 0;
        assert!(ev.validate().is_err());
        ev.flags |= event_flags::TIME_MISSING;
        assert!(ev.validate().is_ok());
        // 非 0 时刻不得打缺时刻位。
        let mut ev = sample();
        ev.flags |= event_flags::TIME_MISSING;
        assert!(ev.validate().is_err());
    }

    #[test]
    fn unknown_unit_code_rejected() {
        let mut ev = sample();
        ev.unit_code = 0xFFFF;
        assert!(ev.validate().is_err());
        // 0 = 未知允许（存档不进控制）。
        ev.unit_code = 0;
        assert!(ev.validate().is_ok());
    }

    #[test]
    fn value_bit_shapes() {
        // Bool 高位非零拒绝。
        let mut bits = [0u8; 16];
        bits[0] = 2;
        assert!(ObservationValue::decode(ValueTag::Bool, &bits).is_none());
        bits[0] = 0;
        bits[15] = 1;
        assert!(ObservationValue::decode(ValueTag::Bool, &bits).is_none());
        assert_eq!(
            ObservationValue::decode(
                ValueTag::Bool,
                &[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
            ),
            Some(ObservationValue::Bool(true))
        );
        // Bytes16 原样往返。
        let raw = [7u8; 16];
        assert_eq!(
            ObservationValue::decode(ValueTag::Bytes16, &raw),
            Some(ObservationValue::Bytes16(raw))
        );
    }

    #[test]
    fn quality_bits_control_eligible() {
        use quality_bits::*;
        assert!(is_control_eligible(VALID));
        assert!(!is_control_eligible(0));
        assert!(!is_control_eligible(VALID | INVALID));
        assert!(!is_control_eligible(VALID | BLOCKED));
        assert!(!is_control_eligible(VALID | OVERFLOW));
        // uncertain/old/substituted 不自动失格（由 descriptor 最低质量裁决）。
        assert!(is_control_eligible(VALID | UNCERTAIN));
    }

    #[test]
    fn integrity_tag_codes() {
        for (i, t) in [
            IntegrityTag::Unverified,
            IntegrityTag::Authenticated,
            IntegrityTag::Signed,
            IntegrityTag::LocalTrustedEdge,
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(t.code() as usize, i);
            assert_eq!(IntegrityTag::from_code(i as u8), Some(t));
        }
        assert_eq!(IntegrityTag::from_code(4), None);
    }
}
