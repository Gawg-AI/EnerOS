//! MockPointAccess / MockDeviceStateProvider — 测试用模拟（仅 `#[cfg(test)]`）.
//!
//! [`MockPointAccess`] 实现 [`eneros_protocol_abstract::PointAccess`]，记录
//! 每次写入以便测试断言下发值；[`MockDeviceStateProvider`] 实现
//! [`eneros_rtos_cmd_exec::state_provider::DeviceStateProvider`]，返回固定状态.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use eneros_controlbus::{DeviceId, DeviceState};
use eneros_protocol_abstract::{PointAccess, ProtocolError, ProtocolType};
use eneros_rtos_cmd_exec::state_provider::DeviceStateProvider;
use eneros_upa_model::{
    DataPoint, DataSource, DeviceId as UpaDeviceId, PointId, PointQuality, PointType, PointValue,
};

/// 模拟点访问（记录写入，用于验证下发值）.
#[derive(Debug, Default)]
pub struct MockPointAccess {
    written_points: BTreeMap<PointId, PointValue>,
}

impl MockPointAccess {
    /// 创建模拟点访问.
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回指定点最后一次写入的值.
    pub fn last_write(&self, point_id: PointId) -> Option<&PointValue> {
        self.written_points.get(&point_id)
    }

    /// 返回已写入的点 ID 列表（排序）.
    pub fn written_point_ids(&self) -> Vec<PointId> {
        self.written_points.keys().copied().collect()
    }
}

impl PointAccess for MockPointAccess {
    fn read_point(&mut self, point_id: PointId) -> Result<DataPoint, ProtocolError> {
        self.written_points
            .get(&point_id)
            .map(|v| DataPoint {
                point_id,
                device_id: 0,
                name: String::from("mock"),
                description: None,
                point_type: PointType::Analog,
                value: v.clone(),
                quality: PointQuality::good(),
                timestamp_ms: 0,
                source: DataSource::Internal,
                unit: None,
            })
            .ok_or(ProtocolError::PointNotFound)
    }

    fn read_points(&mut self, point_ids: &[PointId]) -> Vec<Result<DataPoint, ProtocolError>> {
        point_ids.iter().map(|&id| self.read_point(id)).collect()
    }

    fn write_point(&mut self, point_id: PointId, value: PointValue) -> Result<(), ProtocolError> {
        self.written_points.insert(point_id, value);
        Ok(())
    }

    fn write_points(&mut self, cmds: &[(PointId, PointValue)]) -> Vec<Result<(), ProtocolError>> {
        cmds.iter()
            .map(|(id, v)| self.write_point(*id, v.clone()))
            .collect()
    }

    fn read_device_points(
        &mut self,
        _device_id: UpaDeviceId,
    ) -> Result<Vec<DataPoint>, ProtocolError> {
        Ok(Vec::new())
    }

    fn protocol_type(&self) -> ProtocolType {
        ProtocolType::Internal
    }
}

/// 模拟设备状态来源（返回固定 DeviceState）.
#[derive(Debug, Default)]
pub struct MockDeviceStateProvider {
    state: DeviceState,
}

impl MockDeviceStateProvider {
    /// 创建模拟状态来源（默认全零状态）.
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置固定设备状态.
    pub fn set_state(&mut self, state: DeviceState) {
        self.state = state;
    }
}

impl DeviceStateProvider for MockDeviceStateProvider {
    fn device_state(&self, _device: DeviceId) -> DeviceState {
        self.state
    }
}
