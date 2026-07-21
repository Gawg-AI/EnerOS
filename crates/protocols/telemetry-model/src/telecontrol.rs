//! 遥控（Telecontrol）.
//!
//! [`Telecontrol`] 表示控制命令数据点，提供 SBO（Select-Before-Operate）状态机
//! 与执行结果跟踪。对应电力四遥中的"遥控"（Telecontrol）。

use alloc::string::String;

use eneros_upa_model::{DeviceId, PointId};

use crate::command::{ControlCommand, ControlExecState};
use crate::quality::QualityFlag;

/// 遥控数据点（控制命令）。
///
/// SBO 状态机：
/// - SBO 模式（`select_before_operate == true`）：`Idle` → `Selected` → `Executing` → `Done`/`Failed`/`Timeout`
/// - 非 SBO 模式：`Idle` → `Executing` → `Done`/`Failed`/`Timeout`
#[derive(Debug, Clone)]
pub struct Telecontrol {
    /// 点唯一标识。
    pub point_id: PointId,
    /// 所属设备 ID。
    pub device_id: DeviceId,
    /// 点名称（人类可读）。
    pub name: String,
    /// 控制命令。
    pub command: ControlCommand,
    /// 数据品质。
    pub quality: QualityFlag,
    /// 时间戳（u64 毫秒，D1）。
    pub timestamp_ms: u64,
    /// 是否需要 SBO（选择-执行）。
    pub select_before_operate: bool,
    /// 执行状态。
    pub exec_state: ControlExecState,
}

impl Telecontrol {
    /// 创建遥控点。
    ///
    /// 默认品质 `Good`，`exec_state` 为 `Idle`。
    pub fn new(
        point_id: PointId,
        device_id: DeviceId,
        name: &str,
        command: ControlCommand,
        sbo: bool,
        now_ms: u64,
    ) -> Self {
        Self {
            point_id,
            device_id,
            name: String::from(name),
            command,
            quality: QualityFlag::Good,
            timestamp_ms: now_ms,
            select_before_operate: sbo,
            exec_state: ControlExecState::Idle,
        }
    }

    /// SBO 步骤 1：选择（`Idle` → `Selected`）。
    ///
    /// - 非 SBO 模式调用返回 `Err("not SBO mode, select not required")`。
    /// - 非 `Idle` 态调用返回 `Err("must be Idle to select")`。
    pub fn select(&mut self) -> Result<(), &'static str> {
        if !self.select_before_operate {
            return Err("not SBO mode, select not required");
        }
        if self.exec_state != ControlExecState::Idle {
            return Err("must be Idle to select");
        }
        self.exec_state = ControlExecState::Selected;
        Ok(())
    }

    /// 执行命令（SBO 步骤 2 或非 SBO 直接执行）。
    ///
    /// - SBO 模式：必须为 `Selected` 态，否则返回 `Err("must be Selected to execute in SBO mode")`。
    /// - 非 SBO 模式：必须为 `Idle` 态，否则返回 `Err("must be Idle to execute")`。
    pub fn execute(&mut self) -> Result<(), &'static str> {
        if self.select_before_operate {
            if self.exec_state != ControlExecState::Selected {
                return Err("must be Selected to execute in SBO mode");
            }
        } else if self.exec_state != ControlExecState::Idle {
            return Err("must be Idle to execute");
        }
        self.exec_state = ControlExecState::Executing;
        Ok(())
    }

    /// 执行完成（`Executing` → `Done`）。
    pub fn complete(&mut self) {
        self.exec_state = ControlExecState::Done;
    }

    /// 执行失败（`Executing` → `Failed`）。
    pub fn fail(&mut self) {
        self.exec_state = ControlExecState::Failed;
    }

    /// 执行超时（`Executing` → `Timeout`）。
    pub fn timeout(&mut self) {
        self.exec_state = ControlExecState::Timeout;
    }

    /// 返回是否已完成（`exec_state == Done`）。
    pub fn is_complete(&self) -> bool {
        self.exec_state == ControlExecState::Done
    }
}
