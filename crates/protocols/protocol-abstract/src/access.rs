//! PointAccess trait — 统一点读写访问接口.
//!
//! [`PointAccess`] 是协议抽象层最底层的访问 trait，定义单点读写的核心能力。
//! 所有协议适配器（[`crate::adapter::ProtocolAdapter`]）必须实现本 trait。
//!
//! # 收缩说明（ADR-0026 / ARCH-P03）
//!
//! interface 由真实适配器（modbus-rtu/iec104-master）反推收缩：
//! - 主 trait 只保留真实适配器主路径使用的方法：`read_point`、`write_point`、
//!   `protocol_type`，以及基于单点读的默认批量读 `read_points`；
//! - 批量写 `write_points` 与按设备读 `read_device_points` 移入
//!   [`PointAccessExt`] extension trait（modbus 主路径用单写，按设备读是
//!   上层聚合语义），实现方按需实现。
//!
//! # no_std 合规
//!
//! 不要求 `Send + Sync`（D2：no_std 单线程无需该约束）。

use alloc::boxed::Box;
use alloc::vec::Vec;

use eneros_upa_model::{DataPoint, DeviceId, PointId, PointValue};

use crate::config::ProtocolType;
use crate::error::ProtocolError;

/// 统一点访问接口（读/写/批量读/协议类型）.
///
/// 不要求 `Send + Sync`（D2）；不实现 `subscribe`/`unsubscribe`（D3）。
pub trait PointAccess {
    /// 读取单点当前值。
    fn read_point(&mut self, point_id: PointId) -> Result<DataPoint, ProtocolError>;

    /// 写入单点值。
    fn write_point(&mut self, point_id: PointId, value: PointValue) -> Result<(), ProtocolError>;

    /// 返回该访问接口对应的协议类型。
    fn protocol_type(&self) -> ProtocolType;

    /// 批量读取多点（默认逐点路由，失败项以 `Err` 返回，不影响其他点）。
    fn read_points(&mut self, point_ids: &[PointId]) -> Vec<Result<DataPoint, ProtocolError>> {
        point_ids.iter().map(|&id| self.read_point(id)).collect()
    }
}

/// 点访问扩展 trait（ADR-0026 / ARCH-P03：由主 trait 移出的上层语义方法）。
///
/// 实现方按需实现；默认批量写为逐点写。
pub trait PointAccessExt: PointAccess {
    /// 批量写入多点（默认逐点执行，失败项以 `Err` 返回）。
    fn write_points(&mut self, cmds: &[(PointId, PointValue)]) -> Vec<Result<(), ProtocolError>> {
        cmds.iter()
            .map(|(id, v)| self.write_point(*id, v.clone()))
            .collect()
    }

    /// 读取指定设备下所有点。
    fn read_device_points(&mut self, device_id: DeviceId) -> Result<Vec<DataPoint>, ProtocolError>;
}

/// `Box<dyn PointAccess>` 透传实现（ARCH-P02：注册表持 `Box<dyn PointAccess>` 时
/// 可直接调用点访问方法）。
impl PointAccess for Box<dyn PointAccess + '_> {
    fn read_point(&mut self, point_id: PointId) -> Result<DataPoint, ProtocolError> {
        (**self).read_point(point_id)
    }

    fn write_point(&mut self, point_id: PointId, value: PointValue) -> Result<(), ProtocolError> {
        (**self).write_point(point_id, value)
    }

    fn protocol_type(&self) -> ProtocolType {
        (**self).protocol_type()
    }

    fn read_points(&mut self, point_ids: &[PointId]) -> Vec<Result<DataPoint, ProtocolError>> {
        (**self).read_points(point_ids)
    }
}
