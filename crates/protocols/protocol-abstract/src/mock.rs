//! MockAdapter — 测试用模拟适配器（仅 `#[cfg(test)]`）.
//!
//! [`MockAdapter`] 实现 [`crate::adapter::ProtocolAdapter`] + [`crate::access::AuthorizedWriter`]，
//! 内部用 `BTreeMap<PointId, DataPoint>` 模拟点表，`poll` 仅自增计数器，
//! 用于 [`crate::manager::ProtocolManager`] 与 trait 契约的单元/集成测试。
//!
//! 写接缝（蓝图/24 D1）：`write_authorized` 只接受 [`AuthorizedCommand`]——
//! 设备/点不匹配 → `CommandMismatch`；载荷类型与点类型矛盾 → 拒绝（篡改防御）。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use eneros_telemetry_model::command::AuthorizedCommand;
use eneros_upa_model::{DataPoint, DeviceId, PointId, PointType, PointValue};

use crate::access::{AuthorizedWriter, PointAccess, PointAccessExt, WriteError, WriteOutcome};
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
    /// 关联设备 ID（init 时从配置读取；写接缝匹配用）。
    device_id: DeviceId,
}

impl MockAdapter {
    /// 创建模拟适配器（初始状态 `Uninitialized`）。
    pub fn new(protocol_type: ProtocolType) -> Self {
        Self {
            points: BTreeMap::new(),
            state: AdapterState::Uninitialized,
            protocol_type,
            poll_count: 0,
            device_id: 0,
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

    fn protocol_type(&self) -> ProtocolType {
        self.protocol_type
    }
}

impl PointAccessExt for MockAdapter {
    fn read_device_points(&mut self, device_id: DeviceId) -> Result<Vec<DataPoint>, ProtocolError> {
        let result: Vec<DataPoint> = self
            .points
            .values()
            .filter(|p| p.device_id == device_id)
            .cloned()
            .collect();
        Ok(result)
    }
}

impl AuthorizedWriter for MockAdapter {
    fn write_authorized(
        &mut self,
        cmd: &AuthorizedCommand,
        value: PointValue,
    ) -> Result<WriteOutcome, WriteError> {
        // 1. 设备匹配（蓝图/24 §6.1：不匹配 → CommandMismatch，状态不变）。
        if cmd.device_id() != self.device_id {
            return Err(WriteError::CommandMismatch);
        }
        // 2. 点匹配。
        let point = self
            .points
            .get_mut(&cmd.point_id())
            .ok_or(WriteError::CommandMismatch)?;
        // 3. 篡改防御：载荷值类型与点类型矛盾 → 拒绝（类型级不一致）。
        let type_ok = match (&point.point_type, &value) {
            (PointType::Analog, PointValue::Float(_) | PointValue::Int(_)) => true,
            (PointType::Analog, _) => false,
            (
                PointType::Digital | PointType::Control,
                PointValue::Bool(_) | PointValue::Enum(_),
            ) => true,
            (PointType::Digital | PointType::Control, _) => false,
            (PointType::Setpoint, PointValue::Float(_) | PointValue::Int(_)) => true,
            (PointType::Setpoint, _) => false,
            (PointType::Counter, PointValue::Int(_)) => true,
            (PointType::Counter, _) => false,
        };
        if !type_ok {
            return Err(WriteError::Unauthorized);
        }
        // 4. 落写 + 回读确认（mock 可本地确认）。
        let echoed = value.clone();
        point.value = value;
        Ok(WriteOutcome {
            echoed: Some(echoed),
        })
    }
}

impl ProtocolAdapter for MockAdapter {
    fn init(&mut self, config: &AdapterConfig) -> Result<(), ProtocolError> {
        // 读接缝匹配用的设备 ID（与真实适配器一致：取自 device_configs[0]）。
        self.device_id = config
            .device_configs
            .first()
            .map(|d| d.device_id)
            .unwrap_or(0);
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
