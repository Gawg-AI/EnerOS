//! `points` 点表 target 侧解码面（蓝图 45 WP-45.1）。
//!
//! host 编译面（`toml_compile::compile_points`）把 `configs/points.toml`
//! 编译为 `FIELD_POINTS` 自描述子格式（`schema::points` 注释为单一事实）；
//! 本模块在 no_std 解码面按同一布局严格解码：拒绝未知协议判别码、地址
//! 长度不符、截断、重复 point_id、scale=0、period 越界——任何歧义 → 错误，
//! 不得静默默认。
//!
//! 类型化 consumer（`PointsConsumer`）供 svc-config 注册：registry 双向
//! 完备性要求 bundle 含 points 条目即有对应消费（否则 `UnconsumedConfig`）。
//!
//! 点模型复用 `eneros_upa_model`（PointId/DeviceId/PointType），地址直映
//! `ProtocolAddress`（`protocol-abstract`）——本模块不新造点/地址模型。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_protocol_abstract::ProtocolAddress;
use eneros_protocol_abstract::ProtocolPointMapping;
use eneros_upa_model::{DeviceId, PointId, PointType};

use crate::schema::points;
use crate::{ApplyOutcome, ConfigError, ConfigId, SchemaVersion, TypedConfigConsumer};

/// 协议路由判别（point.toml `protocol` 枚举直映；插件路由，非点模型）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointProtocol {
    ModbusRtu,
    ModbusTcp,
    Iec104,
    Mms,
    Goose,
    Sv,
    Mqtt,
}

impl PointProtocol {
    /// tag → 枚举；未知 → None（解码期 fail-closed）。
    pub fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            points::PROTO_MODBUS_RTU => Some(Self::ModbusRtu),
            points::PROTO_MODBUS_TCP => Some(Self::ModbusTcp),
            points::PROTO_IEC104 => Some(Self::Iec104),
            points::PROTO_MMS => Some(Self::Mms),
            points::PROTO_GOOSE => Some(Self::Goose),
            points::PROTO_SV => Some(Self::Sv),
            points::PROTO_MQTT => Some(Self::Mqtt),
            _ => None,
        }
    }

    /// points.toml `protocol` 字符串 → 枚举；未知 → None。
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "modbus-rtu" => Some(Self::ModbusRtu),
            "modbus-tcp" => Some(Self::ModbusTcp),
            "iec104" => Some(Self::Iec104),
            "mms" => Some(Self::Mms),
            "goose" => Some(Self::Goose),
            "sv" => Some(Self::Sv),
            "mqtt" => Some(Self::Mqtt),
            _ => None,
        }
    }

    /// 枚举 → tag（子格式编码）。
    pub fn tag(self) -> u8 {
        match self {
            Self::ModbusRtu => points::PROTO_MODBUS_RTU,
            Self::ModbusTcp => points::PROTO_MODBUS_TCP,
            Self::Iec104 => points::PROTO_IEC104,
            Self::Mms => points::PROTO_MMS,
            Self::Goose => points::PROTO_GOOSE,
            Self::Sv => points::PROTO_SV,
            Self::Mqtt => points::PROTO_MQTT,
        }
    }

    /// 插件 id（协议服务路由；goose/sv 为订阅型）。
    pub fn plugin_id(self) -> &'static str {
        match self {
            Self::ModbusRtu => "modbus-rtu",
            Self::ModbusTcp => "modbus-tcp",
            Self::Iec104 => "iec104",
            Self::Mms => "mms",
            Self::Goose => "goose",
            Self::Sv => "sv",
            Self::Mqtt => "mqtt",
        }
    }
}

/// 单条点记录（channel wire 解码结果）。
#[derive(Debug, Clone, PartialEq)]
pub struct PointRecord {
    /// 点唯一标识（全表唯一）。
    pub point_id: PointId,
    /// 所属设备 ID。
    pub device_id: DeviceId,
    /// 协议路由。
    pub protocol: PointProtocol,
    /// 协议地址（goose/sv 订阅绑定 → None；其余必须 Some）。
    pub addr: Option<ProtocolAddress>,
    /// 点类型（upa-model PointType，四遥分类）。
    pub data_type: PointType,
    /// 缩放系数（raw → engineering；禁 0）。
    pub scale: f64,
    /// 零点偏移（raw → engineering）。
    pub offset: f64,
    /// 采集周期（毫秒；调度表输入）。
    pub period_ms: u32,
    /// 工程量死区（45.2：deadband 内不刷新缓存）。
    pub deadband: f64,
    /// 值域限幅（45.2：超限 → uncertain）。
    pub range: Option<(f64, f64)>,
}

impl PointRecord {
    /// 复用既有 `ProtocolPointMapping`（工程量变换原语，禁重造）：
    /// 地址型点 → Some(映射)；goose/sv 订阅点 → None（45.4 按数据集绑定）。
    pub fn to_mapping(&self) -> Option<ProtocolPointMapping> {
        self.addr.as_ref().map(|a| ProtocolPointMapping {
            point_id: self.point_id,
            device_id: self.device_id,
            protocol_addr: a.clone(),
            data_type: self.data_type,
            scale: self.scale,
            offset: self.offset,
        })
    }
}

/// 解码后的点表。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PointsTable {
    /// 全部点（保持 wire 顺序）。
    pub points: Vec<PointRecord>,
}

impl PointsTable {
    /// 按 point_id 查找。
    pub fn find(&self, point_id: PointId) -> Option<&PointRecord> {
        self.points.iter().find(|p| p.point_id == point_id)
    }

    /// 按协议路由过滤。
    pub fn by_protocol(&self, protocol: PointProtocol) -> impl Iterator<Item = &PointRecord> {
        self.points.iter().filter(move |p| p.protocol == protocol)
    }

    /// 点个数。
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// 点表是否为空。
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}

// ---------------------------------------------------------------------------
// 严格解码（no_std + alloc；与 schema::points 模块注释布局逐字节一致）
// ---------------------------------------------------------------------------

fn subfield<'a>(
    buf: &'a [u8],
    pos: &mut usize,
    len: usize,
    config: &ConfigId,
    what: &'static str,
) -> Result<&'a [u8], ConfigError> {
    if *pos + len > buf.len() {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "points",
            reason: alloc::format!("truncated:{what}"),
        });
    }
    let out = &buf[*pos..*pos + len];
    *pos += len;
    Ok(out)
}

/// 解码 `FIELD_POINTS` 子格式为点表（严格；任何歧义 → 错误）。
///
/// 输入为 `FIELD_POINTS` 字段的**值**（不含 `field_id|len` 帧头）；
/// canonical payload 完整解码走 [`decode_points`]。
pub(crate) fn decode_points_table(
    raw: &[u8],
    config: &ConfigId,
) -> Result<PointsTable, ConfigError> {
    let mut pos = 0usize;
    let count_buf = subfield(raw, &mut pos, 4, config, "count")?;
    let count = u32::from_le_bytes(count_buf.try_into().unwrap());
    if count > points::MAX_POINTS {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "points",
            reason: "too-many-points".into(),
        });
    }
    let mut out = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let record = decode_record(raw, &mut pos, config)?;
        // point_id 全表唯一（fail-closed，双保险：编译面已查，解码面再查）。
        if out
            .iter()
            .any(|p: &PointRecord| p.point_id == record.point_id)
        {
            return Err(ConfigError::DuplicateField {
                config: config.clone(),
                field: points::FIELD_POINTS,
            });
        }
        out.push(record);
    }
    if pos != raw.len() {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "points",
            reason: "trailing-bytes".into(),
        });
    }
    Ok(PointsTable { points: out })
}

/// 解码完整 `points` canonical payload（`FIELD_POINTS` 帧 → 子格式）。
pub fn decode_points(raw: &[u8], config: &ConfigId) -> Result<PointsTable, ConfigError> {
    let fields = crate::payload::decode_fields(raw, config, points::MAX_FIELD)?;
    let value =
        crate::payload::field(fields.as_slice(), points::FIELD_POINTS).ok_or_else(|| {
            ConfigError::MissingField {
                config: config.clone(),
                field: "points",
            }
        })?;
    decode_points_table(value, config)
}

fn decode_record(
    raw: &[u8],
    pos: &mut usize,
    config: &ConfigId,
) -> Result<PointRecord, ConfigError> {
    let point_id = u32::from_le_bytes(
        subfield(raw, pos, 4, config, "point_id")?
            .try_into()
            .unwrap(),
    );
    let device_id = u16::from_le_bytes(
        subfield(raw, pos, 2, config, "device_id")?
            .try_into()
            .unwrap(),
    );
    let proto_tag = subfield(raw, pos, 1, config, "protocol")?[0];
    let dt_ord = subfield(raw, pos, 1, config, "data_type")?[0];
    let scale = f64::from_le_bytes(subfield(raw, pos, 8, config, "scale")?.try_into().unwrap());
    let offset = f64::from_le_bytes(subfield(raw, pos, 8, config, "offset")?.try_into().unwrap());
    let period_ms = u32::from_le_bytes(
        subfield(raw, pos, 4, config, "period_ms")?
            .try_into()
            .unwrap(),
    );
    let deadband = f64::from_le_bytes(
        subfield(raw, pos, 8, config, "deadband")?
            .try_into()
            .unwrap(),
    );
    let range_flag = subfield(raw, pos, 1, config, "range")?[0];
    let range = match range_flag {
        0 => None,
        1 => {
            let min = f64::from_le_bytes(subfield(raw, pos, 8, config, "min")?.try_into().unwrap());
            let max = f64::from_le_bytes(subfield(raw, pos, 8, config, "max")?.try_into().unwrap());
            // 非有限（NaN/±inf）或 min > max → 拒绝（min≤max 不变量）。
            if !(min.is_finite() && max.is_finite()) || min > max {
                return Err(ConfigError::InvalidValue {
                    config: config.clone(),
                    field: "points",
                    reason: "range-min-gt-max".into(),
                });
            }
            Some((min, max))
        }
        other => {
            return Err(ConfigError::InvalidValue {
                config: config.clone(),
                field: "points",
                reason: alloc::format!("range-flag:{other}"),
            })
        }
    };
    let addr_len = u16::from_le_bytes(
        subfield(raw, pos, 2, config, "addr_len")?
            .try_into()
            .unwrap(),
    ) as usize;
    if addr_len > points::MAX_ADDR_LEN {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "points",
            reason: "addr-too-long".into(),
        });
    }
    let addr_bytes = subfield(raw, pos, addr_len, config, "addr")?;
    let protocol = PointProtocol::from_tag(proto_tag).ok_or_else(|| ConfigError::InvalidValue {
        config: config.clone(),
        field: "points",
        reason: alloc::format!("unknown-protocol:{proto_tag}"),
    })?;
    let data_type = point_type_from_ordinal(dt_ord).ok_or_else(|| ConfigError::InvalidValue {
        config: config.clone(),
        field: "points",
        reason: alloc::format!("unknown-data-type:{dt_ord}"),
    })?;
    if !(scale.is_finite() && scale != 0.0) {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "points",
            reason: "invalid-scale".into(),
        });
    }
    if period_ms == 0 || period_ms > points::MAX_PERIOD_MS {
        return Err(ConfigError::InvalidValue {
            config: config.clone(),
            field: "points",
            reason: "period-out-of-range".into(),
        });
    }
    let addr = decode_addr(protocol, addr_bytes, config)?;
    Ok(PointRecord {
        point_id,
        device_id,
        protocol,
        addr,
        data_type,
        scale,
        offset,
        period_ms,
        deadband,
        range,
    })
}

/// 按协议解码地址字节（与 schema::points 地址编码注释一致）。
fn decode_addr(
    protocol: PointProtocol,
    bytes: &[u8],
    config: &ConfigId,
) -> Result<Option<ProtocolAddress>, ConfigError> {
    let bad_len = |what: &'static str, expected: usize| ConfigError::InvalidValue {
        config: config.clone(),
        field: "points",
        reason: alloc::format!("{what}:addr-len={}/expected={}", bytes.len(), expected),
    };
    let addr = match protocol {
        PointProtocol::ModbusRtu | PointProtocol::ModbusTcp => {
            if bytes.len() != 4 {
                return Err(bad_len("modbus", 4));
            }
            ProtocolAddress::Modbus {
                slave_addr: bytes[0],
                reg_addr: u16::from_le_bytes([bytes[1], bytes[2]]),
                func_code: bytes[3],
            }
        }
        PointProtocol::Iec104 => {
            if bytes.len() != 5 {
                return Err(bad_len("iec104", 5));
            }
            ProtocolAddress::Iec104 {
                common_addr: u16::from_le_bytes([bytes[0], bytes[1]]),
                ioa: u16::from_le_bytes([bytes[2], bytes[3]]),
                type_id: bytes[4],
            }
        }
        PointProtocol::Mms => {
            if bytes.is_empty() {
                return Err(bad_len("mms", 1));
            }
            let object_ref =
                String::from_utf8(bytes.to_vec()).map_err(|_| ConfigError::InvalidValue {
                    config: config.clone(),
                    field: "points",
                    reason: "mms-object-ref-not-utf8".into(),
                })?;
            ProtocolAddress::Mms { object_ref }
        }
        PointProtocol::Mqtt => {
            if bytes.is_empty() {
                return Err(bad_len("mqtt", 1));
            }
            let topic =
                String::from_utf8(bytes.to_vec()).map_err(|_| ConfigError::InvalidValue {
                    config: config.clone(),
                    field: "points",
                    reason: "mqtt-topic-not-utf8".into(),
                })?;
            ProtocolAddress::Mqtt { topic }
        }
        // goose/sv 为订阅绑定：无地址（addr_len 必须为 0）。
        PointProtocol::Goose | PointProtocol::Sv => {
            if !bytes.is_empty() {
                return Err(bad_len("goose-sv", 0));
            }
            return Ok(None);
        }
    };
    Ok(Some(addr))
}

/// 点表 data_type 名 → `PointType`（upa-model 四遥名；小写精确匹配，
/// 禁同义词——"float"/"int"/"bool" 是值类型非四遥分类，拒绝）。
pub(crate) fn point_type_from_name(name: &str) -> Option<PointType> {
    match name {
        "analog" => Some(PointType::Analog),
        "digital" => Some(PointType::Digital),
        "control" => Some(PointType::Control),
        "setpoint" => Some(PointType::Setpoint),
        "counter" => Some(PointType::Counter),
        _ => None,
    }
}

/// upa-model `PointType` 序号（0..=4，按点表子格式）。显式匹配，无 unsafe。
pub(crate) fn point_type_ordinal(point_type: PointType) -> u8 {
    match point_type {
        PointType::Analog => 0,
        PointType::Digital => 1,
        PointType::Control => 2,
        PointType::Setpoint => 3,
        PointType::Counter => 4,
    }
}

fn point_type_from_ordinal(ord: u8) -> Option<PointType> {
    match ord {
        0 => Some(PointType::Analog),
        1 => Some(PointType::Digital),
        2 => Some(PointType::Control),
        3 => Some(PointType::Setpoint),
        4 => Some(PointType::Counter),
        _ => None,
    }
}

/// 类型化 consumer（svc-config 注册；Config = PointsTable）。
#[derive(Debug, Clone, Default)]
pub struct PointsConsumer {
    /// 最近应用的点表。
    pub table: PointsTable,
    /// 生效 generation。
    pub generation: u64,
}

impl TypedConfigConsumer for PointsConsumer {
    type Config = PointsTable;
    fn config_id(&self) -> ConfigId {
        ConfigId::from_static(points::CONFIG_ID)
    }
    fn schema_version(&self) -> SchemaVersion {
        SchemaVersion(points::SCHEMA)
    }
    fn decode_validate(&self, raw: &[u8]) -> Result<Self::Config, ConfigError> {
        decode_points(raw, &ConfigId::from_static(points::CONFIG_ID))
    }
    fn apply(&mut self, cfg: Self::Config, generation: u64) -> Result<ApplyOutcome, ConfigError> {
        if self.table == cfg {
            return Ok(ApplyOutcome::NoOp);
        }
        self.table = cfg;
        self.generation = generation;
        Ok(ApplyOutcome::Applied { generation })
    }
}

#[cfg(test)]
mod tests {
    use eneros_protocol_abstract::ProtocolAddress;

    use super::*;

    fn config() -> ConfigId {
        ConfigId::from_static(points::CONFIG_ID)
    }

    /// 构造一条 modbus-tcp 点 wire 记录段。
    fn record_field_bytes(point_id: u32, slave: u8, reg: u16, func: u8) -> alloc::vec::Vec<u8> {
        let mut b = alloc::vec::Vec::new();
        b.extend_from_slice(&point_id.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.push(points::PROTO_MODBUS_TCP);
        b.push(point_type_ordinal(PointType::Analog));
        b.extend_from_slice(&0.1f64.to_le_bytes());
        b.extend_from_slice(&(-40.0f64).to_le_bytes());
        b.extend_from_slice(&5000u32.to_le_bytes());
        b.extend_from_slice(&0.0f64.to_le_bytes());
        b.push(0); // no range
        b.extend_from_slice(&4u16.to_le_bytes());
        b.push(slave);
        b.extend_from_slice(&reg.to_le_bytes());
        b.push(func);
        b
    }

    #[test]
    fn decode_positive_roundtrip() {
        let mut raw = Vec::new();
        let rec1 = record_field_bytes(1001, 1, 4097, 3);
        raw.extend_from_slice(&1u32.to_le_bytes());
        raw.extend_from_slice(&rec1);
        let table = decode_points_table(&raw, &config()).unwrap();
        assert_eq!(table.len(), 1);
        let p = &table.points[0];
        assert_eq!(p.point_id, 1001);
        assert_eq!(p.protocol, PointProtocol::ModbusTcp);
        assert_eq!(
            p.addr,
            Some(ProtocolAddress::Modbus {
                slave_addr: 1,
                reg_addr: 4097,
                func_code: 3,
            })
        );
        let m = p.to_mapping().unwrap();
        // 复用既有变换原语（45 红线：禁重造）。
        assert_eq!(m.to_engineering(100), 100.0 * 0.1 + -40.0);
    }

    #[test]
    fn decode_rejects_truncated() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&1u32.to_le_bytes());
        let rec = record_field_bytes(1001, 1, 4097, 3);
        raw.extend_from_slice(&rec[..rec.len() - 3]); // 截断
        assert!(decode_points_table(&raw, &config()).is_err());
    }

    #[test]
    fn decode_rejects_duplicate_point_id() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&2u32.to_le_bytes());
        raw.extend_from_slice(&record_field_bytes(1001, 1, 4097, 3));
        raw.extend_from_slice(&record_field_bytes(1001, 2, 4098, 3));
        assert!(decode_points_table(&raw, &config()).is_err());
    }

    #[test]
    fn decode_rejects_unknown_protocol_tag_and_bad_addr() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&1u32.to_le_bytes());
        let mut rec = record_field_bytes(1001, 1, 4097, 3);
        rec[4 + 2] = 99; // protocol tag → 99
        raw.extend_from_slice(&rec);
        assert!(decode_points_table(&raw, &config()).is_err());

        // mqtt 地址长度必须 == topic 字节（给 4B 但填 3 长度 → 截断/错位）。
        let mut raw2 = Vec::new();
        raw2.extend_from_slice(&1u32.to_le_bytes());
        let mut rec2 = Vec::new();
        rec2.extend_from_slice(&1002u32.to_le_bytes());
        rec2.extend_from_slice(&1u16.to_le_bytes());
        rec2.push(points::PROTO_MQTT);
        rec2.push(0);
        rec2.extend_from_slice(&1.0f64.to_le_bytes());
        rec2.extend_from_slice(&0.0f64.to_le_bytes());
        rec2.extend_from_slice(&2000u32.to_le_bytes());
        rec2.extend_from_slice(&0.0f64.to_le_bytes());
        rec2.push(0);
        rec2.extend_from_slice(&4u16.to_le_bytes());
        rec2.extend_from_slice(b"abc"); // 3B < 声明 4B → truncated
        raw2.extend_from_slice(&rec2);
        assert!(decode_points_table(&raw2, &config()).is_err());
    }

    #[test]
    fn decode_goose_sv_subscription_no_addr() {
        for tag in [points::PROTO_GOOSE, points::PROTO_SV] {
            let mut raw = Vec::new();
            raw.extend_from_slice(&1u32.to_le_bytes());
            let mut rec = Vec::new();
            rec.extend_from_slice(&2001u32.to_le_bytes());
            rec.extend_from_slice(&1u16.to_le_bytes());
            rec.push(tag);
            rec.push(point_type_ordinal(PointType::Digital));
            rec.extend_from_slice(&1.0f64.to_le_bytes());
            rec.extend_from_slice(&0.0f64.to_le_bytes());
            rec.extend_from_slice(&1000u32.to_le_bytes());
            rec.extend_from_slice(&0.0f64.to_le_bytes());
            rec.push(0);
            rec.extend_from_slice(&0u16.to_le_bytes());
            raw.extend_from_slice(&rec);
            let table = decode_points_table(&raw, &config()).unwrap();
            assert!(table.points[0].addr.is_none());
            assert!(table.points[0].to_mapping().is_none());
        }
    }

    /// 完整 canonical payload（FIELD_POINTS 帧）解码（consumer 路径）。
    #[test]
    fn decode_full_payload_frame() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&1u32.to_le_bytes());
        raw.extend_from_slice(&record_field_bytes(1001, 1, 4097, 3));
        let payload = crate::payload::encode_payload(&[(points::FIELD_POINTS, raw.as_slice())]);
        let table = decode_points(&payload, &config()).unwrap();
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn consumer_apply_propagates_generation() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&1u32.to_le_bytes());
        raw.extend_from_slice(&record_field_bytes(1001, 1, 4097, 3));
        let payload = crate::payload::encode_payload(&[(points::FIELD_POINTS, raw.as_slice())]);
        let mut c = PointsConsumer::default();
        let table = c.decode_validate(&payload).unwrap();
        let outcome = c.apply(table.clone(), 7).unwrap();
        assert_eq!(outcome, ApplyOutcome::Applied { generation: 7 });
        assert_eq!(c.generation, 7);
        assert_eq!(c.table.len(), 1);
        // 幂等：同表再 apply → NoOp。
        assert_eq!(c.apply(table, 8).unwrap(), ApplyOutcome::NoOp);
    }
}
