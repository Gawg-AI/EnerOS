//! 控制命令与执行状态（Command / ControlExecState）.
//!
//! 定义遥控命令的统一起义与执行状态机，对应 IEC 104 SingleCommand/DoubleCommand
//! 与 Modbus Function Code 5/6/15/16 的归一化语义。

/// 单位置命令（SingleCommand）。
///
/// 对应 IEC 104 SinglePoint 信息体（ON/OFF）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SingleCommand {
    /// 分（OFF）。
    Off,
    /// 合（ON）。
    On,
}

/// 双位置命令（DoubleCommand）。
///
/// 对应 IEC 104 DoublePoint 信息体（OFF/ON/Intermediate/Bad）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoubleCommand {
    /// 分（OFF）。
    Off,
    /// 合（ON）。
    On,
    /// 中间态。
    Intermediate,
    /// 错误。
    Bad,
}

/// 控制命令（统一遥控命令）。
///
/// 包装 `SingleCommand`（单位置）或 `DoubleCommand`（双位置）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCommand {
    /// 单位置命令。
    Single(SingleCommand),
    /// 双位置命令。
    Double(DoubleCommand),
}

/// 控制执行状态机（6 状态）。
///
/// 状态流转：
/// - 非 SBO：`Idle` → `Executing` → `Done`/`Failed`/`Timeout`
/// - SBO：`Idle` → `Selected` → `Executing` → `Done`/`Failed`/`Timeout`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlExecState {
    /// 空闲。
    Idle,
    /// 已选择（SBO 步骤 1）。
    Selected,
    /// 执行中。
    Executing,
    /// 执行完成（终态）。
    Done,
    /// 执行失败（终态）。
    Failed,
    /// 超时（终态）。
    Timeout,
}

impl ControlExecState {
    /// 返回是否为终态（`Done`/`Failed`/`Timeout`）。
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ControlExecState::Done | ControlExecState::Failed | ControlExecState::Timeout
        )
    }

    /// 返回是否为活跃态（`Selected`/`Executing`）。
    pub fn is_active(&self) -> bool {
        matches!(
            self,
            ControlExecState::Selected | ControlExecState::Executing
        )
    }
}
