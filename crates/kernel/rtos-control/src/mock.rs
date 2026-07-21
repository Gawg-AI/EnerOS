//! MockPointAccess — 测试用模拟点访问（仅 `#[cfg(test)]`）.
//!
//! [`MockPointAccess`] 实现 [`eneros_protocol_abstract::PointAccess`]，
//! 内部用 `BTreeMap<PointId, DataPoint>` 模拟点表，用于
//! [`crate::power_loop::PowerControlLoop`] 的单元/集成测试.

use alloc::collections::BTreeMap;
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
}

impl MockPointAccess {
    /// 创建模拟点访问（空点表）.
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置/覆盖模拟点值（f64 浮点）.
    pub fn set_point(&mut self, point_id: PointId, value: f64) {
        let point = DataPoint {
            point_id,
            device_id: 1,
            name: String::from("mock"),
            description: None,
            point_type: PointType::Analog,
            value: PointValue::Float(value),
            quality: PointQuality::good(),
            timestamp_ms: 0,
            source: DataSource::Internal,
            unit: None,
        };
        self.points.insert(point_id, point);
    }
}

impl PointAccess for MockPointAccess {
    fn read_point(&mut self, point_id: PointId) -> Result<DataPoint, ProtocolError> {
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
