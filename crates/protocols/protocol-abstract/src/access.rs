//! PointAccess trait — 统一点读写访问接口.
//!
//! [`PointAccess`] 是协议抽象层最底层的访问 trait，定义单点/批量/按设备的
//! 读写能力。所有协议适配器（[`crate::adapter::ProtocolAdapter`]）必须实现本 trait。
//!
//! # no_std 合规
//!
//! 不要求 `Send + Sync`（D2：no_std 单线程无需该约束）。

use alloc::vec::Vec;

use eneros_upa_model::{DataPoint, DeviceId, PointId, PointValue};

use crate::config::ProtocolType;
use crate::error::ProtocolError;

/// 统一点访问接口（读/写/批量/按设备）.
///
/// 不要求 `Send + Sync`（D2）；不实现 `subscribe`/`unsubscribe`（D3）。
pub trait PointAccess {
    /// 读取单点当前值。
    fn read_point(&mut self, point_id: PointId) -> Result<DataPoint, ProtocolError>;

    /// 批量读取多点（逐点路由，失败项以 `Err` 返回，不影响其他点）。
    fn read_points(&mut self, point_ids: &[PointId]) -> Vec<Result<DataPoint, ProtocolError>>;

    /// 写入单点值。
    fn write_point(&mut self, point_id: PointId, value: PointValue) -> Result<(), ProtocolError>;

    /// 批量写入多点（逐点执行，失败项以 `Err` 返回）。
    fn write_points(&mut self, cmds: &[(PointId, PointValue)]) -> Vec<Result<(), ProtocolError>>;

    /// 读取指定设备下所有点。
    fn read_device_points(&mut self, device_id: DeviceId) -> Result<Vec<DataPoint>, ProtocolError>;

    /// 返回该访问接口对应的协议类型。
    fn protocol_type(&self) -> ProtocolType;
}
