//! EnerOS HMI — 本地人机接口（v0.42.1）
//!
//! 提供 Agent 系统的本地运维接口，包含：
//! - 串口控制台渲染（`console` 模块）
//! - Web 运维接口类型（`web` 模块）
//! - 审批状态机（`approval` 模块）
//!
//! # 偏差声明
//!
//! - **D8**: HMI crate 必须 no_std（`#![cfg_attr(not(test), no_std)]`），
//!   仅使用 `alloc::*` / `core::*`，无 `std::*`。
//! - **D11**: HMI 特有类型（`AgentStateSummary` / `NetworkStatus` / `PowerState` /
//!   `SystemState` / `AlarmSummary` / `AlarmSeverity` / `ManualAction` / `ApprovalId` /
//!   `HmiFrame` / `HmiError`）定义在 hmi crate 而非 eneros-agent crate
//!   （蓝图未明确归属，本实现选择 hmi crate 以保持 eneros-agent 的纯净性）。
//! - **D14**: 审批状态机为内存实现（`BTreeMap`），无持久化
//!   （蓝图未要求持久化，MVP 阶段内存足够）。
//!
//! # no_std 合规
//!
//! 本 crate `#![cfg_attr(not(test), no_std)]` + `extern crate alloc`。
//! 仅使用 `alloc::*` 与 `core::*`。依赖 `eneros-agent`。

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod approval;
pub mod console;
pub mod web;

use alloc::string::String;
use alloc::vec::Vec;

// Re-export approval types
pub use approval::{ApprovalManager, ApprovalState, PendingApproval};
// Re-export console types
pub use console::{ConsoleOutput, ConsoleRenderer};
use eneros_agent::{AgentId, AgentState, AgentType};
// Re-export web types
pub use web::{HttpMethod, HttpRequest, HttpResponse, WebHandler};

/// 审批 ID（u64）
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ApprovalId(pub u64);

/// 告警严重级别（3 级）
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AlarmSeverity {
    /// 信息
    Info,
    /// 警告
    Warning,
    /// 严重
    Critical,
}

/// Agent 状态摘要（HMI 展示用）
#[derive(Clone, Debug, PartialEq)]
pub struct AgentStateSummary {
    /// Agent ID
    pub agent_id: AgentId,
    /// Agent 名称
    pub name: String,
    /// 当前状态
    pub state: AgentState,
    /// Agent 类型
    pub agent_type: AgentType,
}

/// 网络状态
#[derive(Clone, Debug, PartialEq)]
pub struct NetworkStatus {
    /// 是否已连接
    pub connected: bool,
    /// IP 地址（如 "192.168.1.100"）
    pub ip_addr: Option<String>,
    /// 信号强度（dBm，仅无线连接有效）
    pub rssi: Option<i8>,
}

/// 电源状态
#[derive(Clone, Debug, PartialEq)]
pub struct PowerState {
    /// 电池百分比（0-100，AC 连接时为 100）
    pub battery_pct: u8,
    /// 是否正在充电
    pub charging: bool,
    /// 是否连接 AC 电源
    pub ac_connected: bool,
}

/// 系统整体状态（HMI 主视图数据）
#[derive(Clone, Debug, PartialEq)]
pub struct SystemState {
    /// 所有 Agent 的状态摘要
    pub agent_states: Vec<AgentStateSummary>,
    /// 存储使用量（MB）
    pub storage_usage_mb: u32,
    /// 网络状态
    pub network: NetworkStatus,
    /// 电源状态
    pub power: PowerState,
    /// 最后更新时间戳（ms）
    pub last_update_ms: u64,
}

/// 告警摘要
#[derive(Clone, Debug, PartialEq)]
pub struct AlarmSummary {
    /// 告警 ID
    pub id: u64,
    /// 严重级别
    pub severity: AlarmSeverity,
    /// 告警消息
    pub message: String,
    /// 时间戳（ms）
    pub timestamp: u64,
}

/// 手动操作（运维人员触发的动作）
#[derive(Clone, Debug, PartialEq)]
pub struct ManualAction {
    /// 操作 ID
    pub id: u64,
    /// 操作类型（如 "restart_agent" / "suspend_agent" / "clear_alarm"）
    pub action_type: String,
    /// 目标 Agent（None 表示系统级操作）
    pub target_agent: Option<AgentId>,
    /// 操作参数（JSON 字符串，由调用方解析）
    pub params: String,
}

/// HMI 帧（完整界面数据）
#[derive(Clone, Debug, PartialEq)]
pub struct HmiFrame {
    /// 系统状态
    pub system_state: SystemState,
    /// 活跃告警列表
    pub active_alarms: Vec<AlarmSummary>,
    /// 待审批列表
    pub pending_approvals: Vec<PendingApproval>,
    /// 可用手动操作列表
    pub manual_actions: Vec<ManualAction>,
}

/// HMI 错误类型
#[derive(Clone, Debug, PartialEq)]
pub enum HmiError {
    /// 审批不存在
    ApprovalNotFound(ApprovalId),
    /// 非法状态转换
    InvalidStateTransition {
        /// 当前状态
        from: ApprovalState,
        /// 目标状态
        to: ApprovalState,
    },
    /// I/O 错误（控制台输出失败）
    IoError,
    /// 无效请求
    InvalidRequest,
}

/// 渲染 HMI 屏幕（构造 HmiFrame）
///
/// 根据系统状态构造完整的 HMI 帧，包含空告警/审批/操作列表。
pub fn render_hmi_screen(state: &SystemState) -> HmiFrame {
    HmiFrame {
        system_state: state.clone(),
        active_alarms: Vec::new(),
        pending_approvals: Vec::new(),
        manual_actions: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alarm_severity_ordering() {
        assert!(AlarmSeverity::Critical > AlarmSeverity::Warning);
        assert!(AlarmSeverity::Warning > AlarmSeverity::Info);
    }

    #[test]
    fn test_render_hmi_screen_empty() {
        let state = SystemState {
            agent_states: Vec::new(),
            storage_usage_mb: 0,
            network: NetworkStatus {
                connected: false,
                ip_addr: None,
                rssi: None,
            },
            power: PowerState {
                battery_pct: 100,
                charging: false,
                ac_connected: true,
            },
            last_update_ms: 0,
        };
        let frame = render_hmi_screen(&state);
        assert!(frame.active_alarms.is_empty());
        assert!(frame.pending_approvals.is_empty());
        assert!(frame.manual_actions.is_empty());
    }

    #[test]
    fn test_approval_id() {
        let id1 = ApprovalId(1);
        let id2 = ApprovalId(2);
        assert_ne!(id1, id2);
        assert_eq!(id1, ApprovalId(1));
    }

    #[test]
    fn test_hmi_error_variants() {
        let e1 = HmiError::ApprovalNotFound(ApprovalId(42));
        let e2 = HmiError::IoError;
        let e3 = HmiError::InvalidRequest;
        assert_ne!(e1, e2);
        assert_ne!(e2, e3);
        assert_eq!(e1, HmiError::ApprovalNotFound(ApprovalId(42)));
    }
}
