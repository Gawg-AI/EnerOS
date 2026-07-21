//! 设备类型与状态结构 — DeviceType / DeviceState / DeviceSnapshot.

use alloc::collections::BTreeMap;
use alloc::string::String;

/// 设备类型（5 种）.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceType {
    /// PCS（功率变换系统）.
    Pcs,
    /// 电池.
    Battery,
    /// BMS（电池管理系统）.
    Bms,
    /// 电表.
    Meter,
    /// 温度传感器.
    Temperature,
}

/// 设备状态（采集快照）.
#[derive(Debug, Clone, Default)]
pub struct DeviceState {
    /// 荷电状态（0.0~1.0）.
    pub soc: f64,
    /// 电压（V）.
    pub voltage: f64,
    /// 电流（A）.
    pub current: f64,
    /// 温度（℃）.
    pub temperature: f64,
    /// 功率（kW）.
    pub power: f64,
    /// 是否在线.
    pub online: bool,
    /// 最后更新时间戳（ms，外部提供）.
    pub last_update_ms: u64,
}

/// 设备状态快照（多设备聚合，D5：替代 SharedMemoryHandle）.
#[derive(Debug, Clone)]
pub struct DeviceSnapshot {
    /// 设备名 → 状态映射.
    pub states: BTreeMap<String, DeviceState>,
}

impl DeviceSnapshot {
    /// 创建空快照.
    pub fn new() -> Self {
        Self {
            states: BTreeMap::new(),
        }
    }

    /// 设置设备状态.
    pub fn set(&mut self, name: &str, state: DeviceState) {
        self.states.insert(String::from(name), state);
    }

    /// 获取设备状态.
    pub fn get(&self, name: &str) -> Option<&DeviceState> {
        self.states.get(name)
    }

    /// 设备数量.
    pub fn len(&self) -> usize {
        self.states.len()
    }

    /// 是否为空.
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }
}

impl Default for DeviceSnapshot {
    fn default() -> Self {
        Self::new()
    }
}
