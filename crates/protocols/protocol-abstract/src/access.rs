//! PointAccess trait — 统一点读取访问接口 + AuthorizedWriter 写接缝（蓝图/24 D1）.
//!
//! [`PointAccess`] 是协议抽象层最底层的**读**访问 trait。写能力已按
//! 蓝图/24 D1（读写信任分级）移出：读幂等无副作用（任何消费方可用），
//! 写改变物理世界状态，只能经 [`AuthorizedWriter`] 施加——该 trait 只接受
//! [`AuthorizedCommand`]（控制链裁决产物），拿不到授权命令 = 写不出代码
//! （编译期强制，§1.2 推论 C）。
//!
//! # 收缩说明（ADR-0026 / ARCH-P03 / 蓝图/24 D1）
//!
//! - 主 trait 只保留读方法：`read_point`、`protocol_type`，以及基于单点读的
//!   默认批量读 `read_points`；
//! - `write_point`/`write_points` 已**移除**（无可绕过的写旁路）；
//! - 按设备读 `read_device_points` 保留在 [`PointAccessExt`] extension trait。
//!
//! # no_std 合规
//!
//! 不要求 `Send + Sync`（D2：no_std 单线程无需该约束）。

use alloc::boxed::Box;
use alloc::vec::Vec;

use eneros_telemetry_model::command::AuthorizedCommand;
use eneros_upa_model::{DataPoint, DeviceId, PointId, PointValue};

use crate::config::ProtocolType;
use crate::error::ProtocolError;

/// 统一点访问接口（**纯读**；蓝图/24 D1 读写信任分级）。
///
/// 不要求 `Send + Sync`（D2）；不实现 `subscribe`/`unsubscribe`（D3）。
pub trait PointAccess {
    /// 读取单点当前值。
    fn read_point(&mut self, point_id: PointId) -> Result<DataPoint, ProtocolError>;

    /// 返回该访问接口对应的协议类型。
    fn protocol_type(&self) -> ProtocolType;

    /// 批量读取多点（默认逐点路由，失败项以 `Err` 返回，不影响其他点）。
    fn read_points(&mut self, point_ids: &[PointId]) -> Vec<Result<DataPoint, ProtocolError>> {
        point_ids.iter().map(|&id| self.read_point(id)).collect()
    }
}

/// 点访问扩展 trait（ADR-0026 / ARCH-P03：上层聚合语义方法）。
///
/// 实现方按需实现。
pub trait PointAccessExt: PointAccess {
    /// 读取指定设备下所有点。
    fn read_device_points(&mut self, device_id: DeviceId) -> Result<Vec<DataPoint>, ProtocolError>;
}

/// 写接缝错误（蓝图/24 §6.1）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteError {
    /// cmd 的点/设备与本适配器不匹配（适配器拒绝，设备状态不变）。
    CommandMismatch,
    /// 命令未通过复验（上游已验则不应出现，防御性兜底）。
    Unauthorized,
    /// 底层协议写失败。
    Protocol(ProtocolError),
}

/// 授权写结果（蓝图/24 §6.1）。
///
/// `echoed` 为回读确认值，**可空**：适配器可返回本次写入的确认值
/// （如 Mock 直接回显），真实协议适配器以协议应答为准时返回 `None`。
#[derive(Debug, Clone, PartialEq)]
pub struct WriteOutcome {
    /// 回读确认值（可空）。
    pub echoed: Option<PointValue>,
}

/// 写接缝（蓝图/24 D1）——**高信任**，只接受控制链裁决产物。
///
/// 类型系统强制：`write_authorized` 只接受 [`AuthorizedCommand`]；
/// [`AuthorizedCommand`] 只能由控制网关（`ControlGateway::authorize`）产出。
/// 因此"无授权写调用"在编译期不可构造（§1.2 推论 C / 附录 E DoD 3）。
pub trait AuthorizedWriter {
    /// 施加一次授权写。`cmd` 的 device_id/point_id 必须先与本适配器
    /// 匹配（不匹配 → [`WriteError::CommandMismatch`]，设备状态不变）；
    /// 值类型与点类型矛盾（篡改防御）→ 拒绝。成功后返回回读确认
    /// （可空，见 [`WriteOutcome`]）。
    fn write_authorized(
        &mut self,
        cmd: &AuthorizedCommand,
        value: PointValue,
    ) -> Result<WriteOutcome, WriteError>;
}

/// `Box<dyn PointAccess>` 透传实现（ARCH-P02：注册表持 `Box<dyn PointAccess>` 时
/// 可直接调用读方法）。
impl PointAccess for Box<dyn PointAccess + '_> {
    fn read_point(&mut self, point_id: PointId) -> Result<DataPoint, ProtocolError> {
        (**self).read_point(point_id)
    }

    fn protocol_type(&self) -> ProtocolType {
        (**self).protocol_type()
    }

    fn read_points(&mut self, point_ids: &[PointId]) -> Vec<Result<DataPoint, ProtocolError>> {
        (**self).read_points(point_ids)
    }
}

/// `Box<dyn AuthorizedWriter>` 透传实现（写接缝同样可装箱透传）。
impl AuthorizedWriter for Box<dyn AuthorizedWriter + '_> {
    fn write_authorized(
        &mut self,
        cmd: &AuthorizedCommand,
        value: PointValue,
    ) -> Result<WriteOutcome, WriteError> {
        (**self).write_authorized(cmd, value)
    }
}
