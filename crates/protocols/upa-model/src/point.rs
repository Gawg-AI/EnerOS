//! UPA 统一数据点类型.
//!
//! 定义 [`DataPoint`] 及其关联类型（`PointType`/`PointValue`/`PointQuality`/`DataSource`），
//! 将 Modbus/IEC 104/CAN 等协议数据归一化为统一表示。

use alloc::string::String;

/// 点唯一标识（全局唯一，u32 自增分配）。
pub type PointId = u32;

/// 所属设备 ID。
pub type DeviceId = u16;

/// 点类型（四遥分类）。
///
/// 派生 `Ord` 以便作为 `BTreeMap` 的 key（type_index，D4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PointType {
    /// 遥测模拟量（温度/电压/电流/功率等）。
    Analog,
    /// 遥信状态量（开关状态/告警状态）。
    Digital,
    /// 遥控开关量（控制命令）。
    Control,
    /// 遥调设定值（设定参数）。
    Setpoint,
    /// 计数量（电度/电能）。
    Counter,
}

/// 点值（统一值类型）。
///
/// 仅派生 `PartialEq`，不派生 `Eq`，因 `f64` 不实现 `Eq`（D7）。
#[derive(Debug, Clone, PartialEq)]
pub enum PointValue {
    /// 浮点值（遥测模拟量）。
    Float(f64),
    /// 整数值。
    Int(i64),
    /// 布尔值（遥信状态）。
    Bool(bool),
    /// 枚举值（状态机）。
    Enum(u16),
    /// 字符串。
    String(String),
    /// 空值（未初始化或数据丢失）。
    Null,
}

/// 数据品质标志（七标志位，对应 IEC 104 QualityDescriptor 与 Modbus 通信状态统一）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PointQuality {
    /// 数据有效。
    pub valid: bool,
    /// 数据无效。
    pub invalid: bool,
    /// 数据可疑。
    pub questionable: bool,
    /// 替代值。
    pub substituted: bool,
    /// 溢出。
    pub overflow: bool,
    /// 闭锁。
    pub blocked: bool,
    /// 过时。
    pub outdated: bool,
}

impl PointQuality {
    /// 构造好品质：`{ valid: true, 其余 false }`。
    pub fn good() -> Self {
        Self {
            valid: true,
            ..Default::default()
        }
    }

    /// 构造无效品质：`{ invalid: true, 其余 false }`。
    pub fn invalid() -> Self {
        Self {
            invalid: true,
            ..Default::default()
        }
    }
}

/// 数据来源（采集协议来源）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSource {
    /// Modbus RTU。
    ModbusRtu,
    /// Modbus TCP。
    ModbusTcp,
    /// IEC 60870-5-104。
    Iec104,
    /// CAN 总线。
    Can,
    /// 内部生成。
    Internal,
    /// 人工手动设置。
    Manual,
}

/// 统一数据点（Unified Point Abstraction）。
///
/// 将不同协议（Modbus/IEC 104/CAN）的数据归一化为统一格式。
/// `timestamp_ms` 为注入的 u64 毫秒时间戳（D1）。
#[derive(Debug, Clone)]
pub struct DataPoint {
    /// 点唯一标识（全局唯一）。
    pub point_id: PointId,
    /// 所属设备 ID。
    pub device_id: DeviceId,
    /// 点名称（人类可读）。
    pub name: String,
    /// 点描述。
    pub description: Option<String>,
    /// 点类型。
    pub point_type: PointType,
    /// 当前值。
    pub value: PointValue,
    /// 数据品质。
    pub quality: PointQuality,
    /// 时间戳（数据采集时间，u64 毫秒，D1）。
    pub timestamp_ms: u64,
    /// 采集来源。
    pub source: DataSource,
    /// 工程量单位。
    pub unit: Option<String>,
}
