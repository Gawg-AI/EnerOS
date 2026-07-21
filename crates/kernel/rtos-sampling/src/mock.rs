//! MockPointAccess — 测试用模拟点访问（仅 `#[cfg(test)]`）.
//!
//! [`MockPointAccess`] 实现 [`eneros_protocol_abstract::PointAccess`]，
//! 内部用 `BTreeMap<PointId, DataPoint>` 模拟点表，支持标记特定点读取失败，
//! 用于 [`crate::service::SamplingService`] 的单元/集成测试.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use eneros_protocol_abstract::{PointAccess, ProtocolError, ProtocolType};
use eneros_upa_model::{
    DataPoint, DataSource, DeviceId, PointId, PointQuality, PointType, PointValue,
};

/// 模拟点访问（测试专用）.
#[derive(Default)]
pub struct MockPointAccess {
    /// 模拟点表：point_id → DataPoint.
    points: BTreeMap<PointId, DataPoint>,
    /// 标记读取失败的 point_id 集合.
    fail_set: BTreeSet<PointId>,
}

impl MockPointAccess {
    /// 创建模拟点访问（空点表）.
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置/覆盖模拟点（f64 浮点值 + 品质标志）.
    pub fn set_point(&mut self, point_id: PointId, value: f64, valid: bool) {
        self.set_point_value(point_id, PointValue::Float(value), valid);
    }

    /// 设置/覆盖模拟点（任意 PointValue + 品质标志）.
    pub fn set_point_value(&mut self, point_id: PointId, value: PointValue, valid: bool) {
        let dp = DataPoint {
            point_id,
            device_id: 0,
            name: String::from("mock"),
            description: None,
            point_type: PointType::Analog,
            value,
            quality: if valid {
                PointQuality::good()
            } else {
                PointQuality::invalid()
            },
            timestamp_ms: 0,
            source: DataSource::Internal,
            unit: None,
        };
        self.points.insert(point_id, dp);
    }

    /// 标记指定点读取失败（`read_point` 返回 `ReadFailed`）.
    pub fn fail_on_read(&mut self, point_id: PointId) {
        self.fail_set.insert(point_id);
    }
}

impl PointAccess for MockPointAccess {
    fn read_point(&mut self, point_id: PointId) -> Result<DataPoint, ProtocolError> {
        if self.fail_set.contains(&point_id) {
            return Err(ProtocolError::ReadFailed);
        }
        self.points
            .get(&point_id)
            .cloned()
            .ok_or(ProtocolError::PointNotFound)
    }

    fn read_points(&mut self, point_ids: &[PointId]) -> Vec<Result<DataPoint, ProtocolError>> {
        point_ids.iter().map(|&id| self.read_point(id)).collect()
    }

    fn write_point(&mut self, point_id: PointId, value: PointValue) -> Result<(), ProtocolError> {
        if let Some(point) = self.points.get_mut(&point_id) {
            point.value = value;
            Ok(())
        } else {
            Err(ProtocolError::PointNotFound)
        }
    }

    fn write_points(&mut self, cmds: &[(PointId, PointValue)]) -> Vec<Result<(), ProtocolError>> {
        cmds.iter()
            .map(|(id, v)| self.write_point(*id, v.clone()))
            .collect()
    }

    fn read_device_points(&mut self, device_id: DeviceId) -> Result<Vec<DataPoint>, ProtocolError> {
        let result: Vec<DataPoint> = self
            .points
            .values()
            .filter(|p| p.device_id == device_id)
            .cloned()
            .collect();
        Ok(result)
    }

    fn protocol_type(&self) -> ProtocolType {
        ProtocolType::ModbusRtu
    }
}
