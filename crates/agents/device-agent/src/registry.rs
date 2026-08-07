//! 设备注册表 — PointAccess 适配 + MockPointDevice + DeviceInfo + DeviceRegistry.
//!
//! ARCH-P02（ADR-0026/ADR-0027）：删除私有 `DeviceAdapter` 字符串点名旁路，
//! 统一经 [`eneros_protocol_abstract::access::PointAccess`] 访问设备点；
//! 字符串点名 → `PointId` 的映射由 [`DeviceInfo::point_map`] 承担
//! （真实协议场景由 `ProtocolPointMapping` 构建，见 ADR-0026 §5）。

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;

use eneros_protocol_abstract::access::{PointAccess, PointAccessExt};
use eneros_protocol_abstract::config::ProtocolType;
use eneros_protocol_abstract::error::ProtocolError;
use eneros_upa_model::{
    DataPoint, DataSource, DeviceId, PointId, PointQuality, PointType, PointValue,
};

use crate::device_type::DeviceType;

/// 设备元信息（ARCH-P02：adapter 为 `PointAccess`，字符串点名经 point_map 桥接）.
pub struct DeviceInfo {
    /// 设备类型.
    pub device_type: DeviceType,
    /// 设备点访问适配器（统一协议 seam）.
    pub adapter: Box<dyn PointAccess>,
    /// 字符串点名 → PointId 映射（由 `ProtocolPointMapping` 构建或 mock 自动分配）.
    pub point_map: BTreeMap<String, PointId>,
}

/// Mock 点访问设备（BTreeMap-backed，实现 `PointAccess`）.
///
/// 点值带质量位与时间戳（ADR-0027 canonical 点模型）；`set_offline` 后
/// 读点返回 `invalid` 品质（在线判断由质量位表达，而非独立布尔）。
#[cfg(any(test, feature = "mock"))]
pub struct MockPointDevice {
    /// 点 ID → 数据点.
    points: BTreeMap<PointId, DataPoint>,
    /// 下一个自增点 ID.
    next_id: PointId,
    /// 设备 ID.
    device_id: DeviceId,
    /// 离线标志（读点返回 invalid 品质）.
    offline: bool,
}

#[cfg(any(test, feature = "mock"))]
impl MockPointDevice {
    /// 创建空设备（无点位，默认在线）。
    pub fn new(device_id: DeviceId) -> Self {
        Self {
            points: BTreeMap::new(),
            next_id: 1,
            device_id,
            offline: false,
        }
    }

    /// 添加点位（自动分配 PointId，返回字符串点名 → PointId 映射项）。
    pub fn with_point(mut self, name: &str, value: f64) -> Self {
        let id = self.next_id;
        self.next_id += 1;
        self.points.insert(
            id,
            DataPoint {
                point_id: id,
                device_id: self.device_id,
                name: String::from(name),
                description: None,
                point_type: PointType::Analog,
                value: PointValue::Float(value),
                quality: PointQuality::good(),
                timestamp_ms: 0,
                source: DataSource::Internal,
                unit: None,
            },
        );
        self
    }

    /// 收集点名为 name 的 PointId（供 DeviceInfo.point_map 使用）。
    pub fn point_id_of(&self, name: &str) -> Option<PointId> {
        self.points
            .values()
            .find(|p| p.name == name)
            .map(|p| p.point_id)
    }

    /// 设置点位值（按名称）。
    pub fn set_point(&mut self, name: &str, value: f64) {
        if let Some(id) = self.point_id_of(name) {
            if let Some(p) = self.points.get_mut(&id) {
                p.value = PointValue::Float(value);
            }
        }
    }

    /// 设置离线状态（离线时读点返回 invalid 品质）。
    pub fn set_offline(&mut self, offline: bool) {
        self.offline = offline;
    }

    /// 全部点 ID 列表。
    pub fn point_ids(&self) -> alloc::vec::Vec<PointId> {
        self.points.keys().copied().collect()
    }
}

#[cfg(any(test, feature = "mock"))]
impl PointAccess for MockPointDevice {
    fn read_point(&mut self, point_id: PointId) -> Result<DataPoint, ProtocolError> {
        let mut dp = self
            .points
            .get(&point_id)
            .cloned()
            .ok_or(ProtocolError::PointNotFound)?;
        if self.offline {
            dp.quality = PointQuality::invalid();
        }
        Ok(dp)
    }

    fn write_point(&mut self, point_id: PointId, value: PointValue) -> Result<(), ProtocolError> {
        if self.offline {
            return Err(ProtocolError::WriteFailed);
        }
        let point = self
            .points
            .get_mut(&point_id)
            .ok_or(ProtocolError::PointNotFound)?;
        point.value = value;
        Ok(())
    }

    fn protocol_type(&self) -> ProtocolType {
        ProtocolType::Internal
    }
}

#[cfg(any(test, feature = "mock"))]
impl PointAccessExt for MockPointDevice {
    fn read_device_points(&mut self, device_id: DeviceId) -> Result<alloc::vec::Vec<DataPoint>, ProtocolError> {
        if device_id != self.device_id {
            return Err(ProtocolError::PointNotFound);
        }
        Ok(self.points.values().cloned().collect())
    }
}

/// 设备注册表（BTreeMap-backed）.
pub struct DeviceRegistry {
    /// 设备名 → 设备信息映射.
    devices: BTreeMap<String, DeviceInfo>,
}

impl DeviceRegistry {
    /// 创建空注册表.
    pub fn new() -> Self {
        Self {
            devices: BTreeMap::new(),
        }
    }

    /// 注册设备（adapter 为统一 `PointAccess`；point_map 为字符串点名 → PointId）.
    pub fn register(
        &mut self,
        name: &str,
        device_type: DeviceType,
        adapter: Box<dyn PointAccess>,
        point_map: BTreeMap<String, PointId>,
    ) {
        self.devices.insert(
            String::from(name),
            DeviceInfo {
                device_type,
                adapter,
                point_map,
            },
        );
    }

    /// 获取设备可变引用.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut DeviceInfo> {
        self.devices.get_mut(name)
    }

    /// 设备数量.
    pub fn len(&self) -> usize {
        self.devices.len()
    }

    /// 是否为空.
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    /// 可变迭代.
    pub fn iter_mut(&mut self) -> alloc::collections::btree_map::IterMut<'_, String, DeviceInfo> {
        self.devices.iter_mut()
    }

    /// 只读迭代（v0.227.0，注册表持久化需要）.
    pub fn iter(&self) -> alloc::collections::btree_map::Iter<'_, String, DeviceInfo> {
        self.devices.iter()
    }
}

impl Default for DeviceRegistry {
    fn default() -> Self {
        Self::new()
    }
}
