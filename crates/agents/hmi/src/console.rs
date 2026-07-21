//! 串口控制台渲染 — HMI 文本界面生成（v0.42.1）
//!
//! 提供系统状态、告警、审批项的文本渲染，通过 `ConsoleOutput` trait
//! 抽象输出目标（串口、缓冲区等）。
//!
//! # 偏差声明
//!
//! - **D9**: `ConsoleOutput` trait 抽象 I/O 操作（no_std 无 `std::io::Write`），
//!   允许调用方注入串口/缓冲区实现。
//! - **D13**: `render` 方法返回 `String`（纯文本），VT100 转义码为可选
//!   （当前实现为纯文本，无颜色控制）。
//!
//! # no_std 合规
//!
//! 仅使用 `alloc::*` / `core::*`，无 `std::*`，无 `panic!`/`todo!`/`unimplemented!`。

use alloc::format;
use alloc::string::String;

use crate::{AlarmSeverity, HmiError, HmiFrame, PendingApproval, SystemState};

/// 控制台输出抽象 trait（D9）
///
/// 抽象 I/O 操作，允许调用方注入串口/缓冲区实现。
/// no_std 环境无 `std::io::Write`，故自定义此 trait。
pub trait ConsoleOutput {
    /// 写入字符串
    fn write_str(&mut self, s: &str) -> Result<(), HmiError>;
}

/// 控制台渲染器
///
/// 将 `SystemState` / `HmiFrame` 渲染为文本字符串。
#[derive(Debug, Clone, Default)]
pub struct ConsoleRenderer {
    // 当前无状态，未来可扩展（如宽度、颜色开关等）
}

impl ConsoleRenderer {
    /// 创建控制台渲染器
    pub fn new() -> Self {
        Self::default()
    }

    /// 渲染系统状态为文本（D13：纯文本，无 VT100）
    pub fn render(&self, state: &SystemState) -> String {
        let mut output = String::new();

        output.push_str("=== EnerOS System Status ===\n");
        output.push_str(&format!("Last Update: {} ms\n", state.last_update_ms));
        output.push_str(&format!("Storage: {} MB\n", state.storage_usage_mb));

        // Network
        output.push_str(&format!(
            "Network: {}",
            if state.network.connected {
                "Connected"
            } else {
                "Disconnected"
            }
        ));
        if let Some(ref ip) = state.network.ip_addr {
            output.push_str(&format!(" ({})", ip));
        }
        if let Some(rssi) = state.network.rssi {
            output.push_str(&format!(" RSSI={}dBm", rssi));
        }
        output.push('\n');

        // Power
        output.push_str(&format!("Power: Battery {}%", state.power.battery_pct));
        if state.power.ac_connected {
            output.push_str(" [AC]");
        } else if state.power.charging {
            output.push_str(" [Charging]");
        }
        output.push('\n');

        // Agents
        output.push_str(&format!("Agents ({}):\n", state.agent_states.len()));
        for agent in &state.agent_states {
            output.push_str(&format!(
                "  [{:?}] {} (id={:?}) {:?}\n",
                agent.agent_type, agent.name, agent.agent_id, agent.state
            ));
        }

        output
    }

    /// 渲染完整 HMI 帧为文本
    pub fn render_frame(&self, frame: &HmiFrame) -> String {
        let mut output = self.render(&frame.system_state);

        // Alarms
        if !frame.active_alarms.is_empty() {
            output.push_str(&format!(
                "\n=== Alarms ({}) ===\n",
                frame.active_alarms.len()
            ));
            for alarm in &frame.active_alarms {
                let severity_str = match alarm.severity {
                    AlarmSeverity::Info => "INFO",
                    AlarmSeverity::Warning => "WARN",
                    AlarmSeverity::Critical => "CRIT",
                };
                output.push_str(&format!(
                    "  [{}] #{}: {} @ {}ms\n",
                    severity_str, alarm.id, alarm.message, alarm.timestamp
                ));
            }
        }

        // Pending approvals
        if !frame.pending_approvals.is_empty() {
            output.push_str(&format!(
                "\n=== Pending Approvals ({}) ===\n",
                frame.pending_approvals.len()
            ));
            output.push_str(&self.render_approvals(&frame.pending_approvals));
        }

        // Manual actions
        if !frame.manual_actions.is_empty() {
            output.push_str(&format!(
                "\n=== Manual Actions ({}) ===\n",
                frame.manual_actions.len()
            ));
            for action in &frame.manual_actions {
                output.push_str(&format!(
                    "  #{}: {} target={:?} params={}\n",
                    action.id, action.action_type, action.target_agent, action.params
                ));
            }
        }

        output
    }

    /// 渲染审批列表为文本
    pub fn render_approvals(&self, approvals: &[PendingApproval]) -> String {
        let mut output = String::new();
        for approval in approvals {
            output.push_str(&format!(
                "  [{:?}] #{}: {} by {} @ {}ms\n",
                approval.state,
                approval.id.0,
                approval.action.action_type,
                approval.requester,
                approval.timestamp
            ));
        }
        output
    }

    /// 渲染并写入到 ConsoleOutput
    pub fn write_to(
        &self,
        state: &SystemState,
        output: &mut dyn ConsoleOutput,
    ) -> Result<(), HmiError> {
        let rendered = self.render(state);
        output.write_str(&rendered)
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::String;
    use alloc::vec;

    use eneros_agent::{AgentId, AgentState, AgentType};

    use super::*;
    use crate::{
        AgentStateSummary, AlarmSeverity, AlarmSummary, HmiFrame, ManualAction, NetworkStatus,
        PowerState, SystemState,
    };

    /// Mock ConsoleOutput that captures written strings
    struct MockOutput {
        buffer: String,
    }

    impl MockOutput {
        fn new() -> Self {
            Self {
                buffer: String::new(),
            }
        }
    }

    impl ConsoleOutput for MockOutput {
        fn write_str(&mut self, s: &str) -> Result<(), HmiError> {
            self.buffer.push_str(s);
            Ok(())
        }
    }

    fn make_state() -> SystemState {
        SystemState {
            agent_states: vec![AgentStateSummary {
                agent_id: AgentId(1),
                name: String::from("test-agent"),
                state: AgentState::Running,
                agent_type: AgentType::System,
            }],
            storage_usage_mb: 500,
            network: NetworkStatus {
                connected: true,
                ip_addr: Some(String::from("192.168.1.100")),
                rssi: Some(-50),
            },
            power: PowerState {
                battery_pct: 85,
                charging: false,
                ac_connected: true,
            },
            last_update_ms: 12345,
        }
    }

    #[test]
    fn test_render_state_basic() {
        let renderer = ConsoleRenderer::new();
        let state = make_state();
        let output = renderer.render(&state);
        assert!(output.contains("EnerOS System Status"));
        assert!(output.contains("Storage: 500 MB"));
        assert!(output.contains("Connected"));
        assert!(output.contains("192.168.1.100"));
        assert!(output.contains("Battery 85%"));
        assert!(output.contains("[AC]"));
        assert!(output.contains("Agents (1):"));
        assert!(output.contains("test-agent"));
    }

    #[test]
    fn test_render_frame_with_alarms() {
        let renderer = ConsoleRenderer::new();
        let state = make_state();
        let frame = HmiFrame {
            system_state: state,
            active_alarms: vec![AlarmSummary {
                id: 1,
                severity: AlarmSeverity::Critical,
                message: String::from("Agent crashed"),
                timestamp: 9999,
            }],
            pending_approvals: vec![],
            manual_actions: vec![],
        };
        let output = renderer.render_frame(&frame);
        assert!(output.contains("=== Alarms (1) ==="));
        assert!(output.contains("[CRIT]"));
        assert!(output.contains("Agent crashed"));
    }

    #[test]
    fn test_render_frame_with_approvals() {
        use crate::{ApprovalId, ApprovalState, PendingApproval};
        let renderer = ConsoleRenderer::new();
        let state = make_state();
        let frame = HmiFrame {
            system_state: state,
            active_alarms: vec![],
            pending_approvals: vec![PendingApproval {
                id: ApprovalId(1),
                action: ManualAction {
                    id: 1,
                    action_type: String::from("restart_agent"),
                    target_agent: None,
                    params: String::from("{}"),
                },
                requester: String::from("operator"),
                timestamp: 1000,
                state: ApprovalState::Pending,
            }],
            manual_actions: vec![],
        };
        let output = renderer.render_frame(&frame);
        assert!(output.contains("=== Pending Approvals (1) ==="));
        assert!(output.contains("[Pending]"));
        assert!(output.contains("restart_agent"));
        assert!(output.contains("operator"));
    }

    #[test]
    fn test_render_empty_state() {
        let renderer = ConsoleRenderer::new();
        let state = SystemState {
            agent_states: vec![],
            storage_usage_mb: 0,
            network: NetworkStatus {
                connected: false,
                ip_addr: None,
                rssi: None,
            },
            power: PowerState {
                battery_pct: 0,
                charging: false,
                ac_connected: false,
            },
            last_update_ms: 0,
        };
        let output = renderer.render(&state);
        assert!(output.contains("Disconnected"));
        assert!(output.contains("Agents (0):"));
    }

    #[test]
    fn test_render_multiple_agents() {
        let renderer = ConsoleRenderer::new();
        let state = SystemState {
            agent_states: vec![
                AgentStateSummary {
                    agent_id: AgentId(1),
                    name: String::from("agent-1"),
                    state: AgentState::Running,
                    agent_type: AgentType::System,
                },
                AgentStateSummary {
                    agent_id: AgentId(2),
                    name: String::from("agent-2"),
                    state: AgentState::Suspended,
                    agent_type: AgentType::Device,
                },
            ],
            storage_usage_mb: 100,
            network: NetworkStatus {
                connected: true,
                ip_addr: None,
                rssi: None,
            },
            power: PowerState {
                battery_pct: 50,
                charging: true,
                ac_connected: false,
            },
            last_update_ms: 5000,
        };
        let output = renderer.render(&state);
        assert!(output.contains("Agents (2):"));
        assert!(output.contains("agent-1"));
        assert!(output.contains("agent-2"));
        assert!(output.contains("[Charging]"));
    }

    #[test]
    fn test_write_to_output() {
        let renderer = ConsoleRenderer::new();
        let state = make_state();
        let mut mock = MockOutput::new();
        let result = renderer.write_to(&state, &mut mock);
        assert!(result.is_ok());
        assert!(mock.buffer.contains("EnerOS System Status"));
    }

    #[test]
    fn test_render_alarm_severity_levels() {
        let renderer = ConsoleRenderer::new();
        let state = make_state();
        let frame = HmiFrame {
            system_state: state,
            active_alarms: vec![
                AlarmSummary {
                    id: 1,
                    severity: AlarmSeverity::Info,
                    message: String::from("info msg"),
                    timestamp: 1,
                },
                AlarmSummary {
                    id: 2,
                    severity: AlarmSeverity::Warning,
                    message: String::from("warn msg"),
                    timestamp: 2,
                },
                AlarmSummary {
                    id: 3,
                    severity: AlarmSeverity::Critical,
                    message: String::from("crit msg"),
                    timestamp: 3,
                },
            ],
            pending_approvals: vec![],
            manual_actions: vec![],
        };
        let output = renderer.render_frame(&frame);
        assert!(output.contains("[INFO]"));
        assert!(output.contains("[WARN]"));
        assert!(output.contains("[CRIT]"));
    }
}
