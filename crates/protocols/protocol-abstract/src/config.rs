//! 协议配置 — ProtocolType / DeviceConfig / AdapterConfig.
//!
//! 定义协议类型枚举（可作 `BTreeMap` key）、设备配置与适配器配置，
//! 为 [`crate::adapter::ProtocolAdapter::init`] 提供入参。

use alloc::string::String;
use alloc::vec::Vec;

use eneros_upa_model::DeviceId;

use crate::address::ProtocolAddress;

/// 协议类型.
///
/// 派生 `Ord` 以便作为 `BTreeMap<ProtocolType, _>` 的 key
/// （`ProtocolManager` 按协议类型索引适配器，D4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ProtocolType {
    /// Modbus RTU（串口）。
    ModbusRtu,
    /// Modbus TCP（以太网）。
    ModbusTcp,
    /// IEC 60870-5-104。
    Iec104,
    /// CAN 总线。
    Can,
    /// 内部生成（不经过外部协议）。
    Internal,
}

/// 设备配置（单设备：ID + 名称 + 协议地址）。
#[derive(Debug, Clone)]
pub struct DeviceConfig {
    /// 设备 ID。
    pub device_id: DeviceId,
    /// 设备名称（人类可读）。
    pub name: String,
    /// 设备协议地址。
    pub address: ProtocolAddress,
}

/// 适配器配置（一组同协议设备）。
#[derive(Debug, Clone)]
pub struct AdapterConfig {
    /// 适配器名称。
    pub name: String,
    /// 协议类型。
    pub protocol_type: ProtocolType,
    /// 该适配器下的设备配置列表。
    pub device_configs: Vec<DeviceConfig>,
}
