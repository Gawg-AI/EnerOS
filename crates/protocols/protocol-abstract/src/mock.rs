//! MockAdapter — 测试用模拟适配器（仅 `#[cfg(test)]`）.
//!
//! [`MockAdapter`] 实现 [`crate::adapter::ProtocolAdapter`]，内部用
//! `BTreeMap<PointId, DataPoint>` 模拟点表，`poll` 仅自增计数器，
//! 用于 [`crate::manager::ProtocolManager`] 与 trait 契约的单元/集成测试。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_upa_model::{DataPoint, DeviceId, PointId, PointValue};

use crate::access::PointAccess;
use crate::adapter::{AdapterState, ProtocolAdapter};
use crate::config::{AdapterConfig, ProtocolType};
use crate::error::ProtocolError;

/// 模拟适配器（测试专用）.
pub struct MockAdapter {
    /// 模拟点表：point_id → DataPoint。
    points: BTreeMap<PointId, DataPoint>,
    /// 当前状态。
    state: AdapterState,
    /// 协议类型。
    protocol_type: ProtocolType,
    /// poll 调用计数。
    poll_count: u32,
}

impl MockAdapter {
    /// 创建模拟适配器（初始状态 `Uninitialized`）。
    pub fn new(protocol_type: ProtocolType) -> Self {
        Self {
            points: BTreeMap::new(),
            state: AdapterState::Uninitialized,
            protocol_type,
            poll_count: 0,
        }
    }

    /// 设置/覆盖模拟点值。
    pub fn set_point(&mut self, point_id: PointId, point: DataPoint) {
        self.points.insert(point_id, point);
    }

    /// 返回 poll 调用次数。
    pub fn poll_count(&self) -> u32 {
        self.poll_count
    }
}

impl PointAccess for MockAdapter {
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
        self.protocol_type
    }
}

impl ProtocolAdapter for MockAdapter {
    fn init(&mut self, _config: &AdapterConfig) -> Result<(), ProtocolError> {
        self.state = AdapterState::Initialized;
        Ok(())
    }

    fn start(&mut self) -> Result<(), ProtocolError> {
        self.state = AdapterState::Running;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), ProtocolError> {
        self.state = AdapterState::Stopped;
        Ok(())
    }

    fn poll(&mut self, _now_ms: u64) -> Result<(), ProtocolError> {
        self.poll_count += 1;
        Ok(())
    }

    fn state(&self) -> AdapterState {
        self.state
    }
}
